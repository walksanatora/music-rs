// a bunch of impports from the standard library. in order...
// we import mutex/once lock for some globals (statics)
// vecDequeue since it is efficent to push/pop from front unlike vec which makes it slow.
// fmt so we can implement Debug on some of our types
// fs so we can read audio files to bytes
// path(buf) for the ability to actually read files
// OsStr is needed for some souvlaki stuff (that or it was pathbuf. it has been soo long)
// process stuff so we can exit early, and command so that we can run `openmpt123` as subprocess
// duration so it can manage delays/times with souvlaki
use std::{sync::{Mutex, OnceLock}, collections::VecDeque, fmt, fs, path::{Path, PathBuf}, ffi::OsStr, process::{exit, Command, Stdio}, time::Duration, str::FromStr};

// we then import clap so making CLI args are easy
use clap::{command, Parser};
// kira is a audio manager crate that allows us to play audio...
use kira::{manager::{AudioManager, backend::DefaultBackend, AudioManagerSettings}, sound::{PlaybackState, static_sound::{StaticSoundData, StaticSoundHandle, StaticSoundSettings}}, tween::Tween};
// I would use more functions from openmpt but the api is broken.
use openmpt::info::get_supported_extensions;
// souvlaki provides cross-platform media controls
use souvlaki::{PlatformConfig, MediaControls, MediaMetadata, MediaControlEvent, MediaPosition, SeekDirection};
// and we use rand to shuffle the list.
use rand::thread_rng;
use rand::seq::SliceRandom;

/// takes a iterator of chars and produces a list of strings that have been surrounded by quotes
fn quoted<T>(tgt: T) -> Vec<String> where T: Iterator<Item = char> {
    let mut res = vec![]; // the result list
    let mut capture = false; //if we are within a "
    let mut buf = vec![]; //the current buffer of chars that will be en-stringed
    let mut escaped = false; // if the next char is escaped (allows file names to contain with a `"` by escaping it)
    for ch in tgt { //over each char
        if escaped { //check if we escaped it
            buf.push(ch); //push char directly
            escaped = false //stop the escape
        } else if ch == '"' { // if char is a quote
            if capture { // check if capture is set
                let str: String = buf.iter().collect(); // collect the buffer into a string
                res.push(str); // add string to the result list
            } else {
                buf.clear() // clear the buffer (since it is "dead" space between words)
            }
            capture = !capture; // invert capture so that on first quote we start capturing, and on second we stop capturing and add string to results
        } else if ch == '\\' { //if char is a \ we escape the next char
            escaped = true
        } else {
            buf.push(ch) // we just push the char since it needs no special handling
        }
    };
    res //return the results
}

/// global struct for the state of the media player
struct Status {
    /// whether or not the media player is paused
    paused: bool,
    /// the instancce of the souvlaki media controlls
    controls: MediaControls,
    /// the instance of the kira audio manager
    manager: AudioManager,
    /// the upcoming list of paths to play as music
    upcoming: VecDeque<PathBuf>,
    /// a size-limited queue that acts as a "lookback" buffer so you can play previous songs
    lookback: VecDeque<PathBuf>,
    /// this is a kira soundhandle. if audio is playing this should be `Some`
    handle: Option<StaticSoundHandle>
}

/// debug formatter for printing status mid-run (ignores the handle and manager and controlls field)
impl fmt::Debug for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Status").field("upcoming", &self.upcoming).field("lookback", &self.lookback).finish()
    }
}

impl Status {
    /// stops playing audio if it is playing.
    fn stopit(&mut self) {
        if self.handle.is_some() {
            self.handle.as_mut().unwrap().stop(Tween::default()).unwrap();
        }
    }
    /// stops the current song and plays the next one 
    fn play_next_song(&mut self) {
        self.stopit();
        println!("playing next song");
        //get the next song or if there is none stop the current song and exit
        let upcoming = if let Some(upcoming) = self.upcoming.pop_front() {
            upcoming //we have a next song
        } else {
            // there is no next song so we just silence the current one and return without the next one (other code detects that handle has stopped)
            let _ = self.handle.as_mut().unwrap().stop(Tween::default());
            return;
        };

        //check if the name starts with a `@` in which case it is a special case
        //special case as for eg: if the song is shuffled but I want these songs to be played in order. eg: Bergentrückung + ASGORE from undertale
        if upcoming.to_string_lossy().starts_with('@') {
            let mut words = quoted(upcoming.to_string_lossy().chars());// split the string into quoted words
            words.reverse();//reverse so they are pushed onto song queue right
            for song in words {
                self.upcoming.push_front(song.into())// put them on here
            };
            self.play_next_song(); // head STRAIGHT to playing the next song
            return; // so we dont push the @ line to the lookback (it gets buggy if we do)
        };

        //push the song to loopback so the back button works
        self.push_song_to_lookback(upcoming.clone());

        //turn the path back so it can be checked 
        let path = Path::new(&upcoming);

        if !path.exists() { // if path does not exists we just exit so it can start next song (or stop the music player if that was the last one)
            println!("Path {path:?} does not exists. Skipping"); // let the user in terminal know that path does not exists
            return
        }

        //get path's extension. or default it to blank if it does not exists/cannot be turned into UTF-8
        let ext = path.extension().unwrap_or(OsStr::new("")).to_str().unwrap_or("");
        
        #[allow(unused_mut)] // create a media metadata, must silence unused mut as a later assign to `meta.duration` fails if I do not define this as mutable
        let mut meta = MediaMetadata {
            title: path.file_name().unwrap().to_str(),
            ..Default::default()
        };

        let sound = match ext {
            "wav" | "mp3" | "flac" | "ogg" => { // known file type that kira supports directly so we play it
                StaticSoundData::from_file(path, StaticSoundSettings::default()).unwrap()
            }
            x if MOD_FORMATS.get().unwrap().contains(&x.to_string()) => { // convert the tracker music to a tmp wav file.
                // this is TEMPORARY until the openmpt crate starts working again
                let mut cmd = Command::new("openmpt123"); // start making a new `openmpt123` command to run in terminal
                cmd.args([path.to_str().unwrap(),"-o","/tmp/openmpt_convert.wav", "--force"]); // convert the selected tracker music file and put it at /tmp/openmpt_convert.wav, and replace it if it allready exists
                cmd.stdout(Stdio::null()); // supress stdout 
                let _ = cmd.spawn().unwrap().wait(); // run the command
                StaticSoundData::from_file("/tmp/openmpt_convert.wav", StaticSoundSettings::default()).unwrap() // load wav file
                
                // the INTENDED method. but the openmpt crate is broken (does not fill buffers correctly)
                //let mut file = File::open(path).unwrap();
                //let module = Module::create(&mut file, Logger::None, &[]).unwrap();
                //StreamingSoundData::from_decoder(ModDecoder::new(module), StreamingSoundSettings::default())
            }
            _ => {
                println!("unsupported format '{}' file {}. SKIPPING",ext,path.to_str().unwrap_or("!!failed to unwrap path as str!!"));
                return; // it failed to play song so we skip to next song
            }
        };

        //set metadata's duration for the song
        meta.duration = Some(sound.duration());

        //create a new static sound handle for the song
        let hand = self.manager.play(sound).unwrap();
        //set the handle for audio
        self.handle = Some(hand);

        let _ = self.controls.set_metadata(meta); //set media metadata
        let _ = self.controls.set_playback(souvlaki::MediaPlayback::Playing { progress: None }); //play the song (with no progress since we have not started)
        println!("song is playing"); // notify user via text that song has started
        
    }
    /// pushes a specified PathBuf to the front of lookback. this voids a old value if the len is == capacity
    fn push_song_to_lookback(&mut self, song: PathBuf) {
        if self.lookback.len() == self.lookback.capacity() {
            let _ = self.lookback.pop_back(); //we know it is at capacity. this makes it so that we clear the last index and prevent it from crashing due to being over full
        }
        self.lookback.push_front(song) // push new song to lookback
    }

    /// plays the song at the front of the lookback...
    fn do_the_previous_one(&mut self) {
        // we pop one from the lookback (the current song)
        if let Some(song) = self.lookback.pop_front() {
            self.upcoming.push_front(song);
        }
        // we pop a second one from the lookback (the previous song)
        if let Some(song) = self.lookback.pop_front() {
            self.upcoming.push_front(song);
        }
        // we then play the next song
        self.play_next_song();
    }
}

#[derive(Parser, Debug)]
#[command(author = "[redacted]", version = "v1", about = "command line music player", long_about = None)]
struct Args {
    /// whether or not to shuffle the audio before playing/when playlist is empty
    #[arg(short, long, help = "sets whether or not to shuffle the music list")]
    shuffle: bool,

    /// whether or not to loop the playlist/song when it is empty/over
    #[arg(short, long, help = "sets looping of the music when all songs have been played")]
    looping: bool,
    
    /// all the songs/playlist to play
    #[arg(required(true))]
    files: Vec<PathBuf>,
}

/// a once lock to hold a mutex of our status so we can refrence and init it later
static GLOBAL_STATE: OnceLock<Mutex<Status>> = OnceLock::new();

/// I *would* do this at compile time. but it can change from platform to platform.
static MOD_FORMATS: OnceLock<Vec<String>> = OnceLock::new();

/// this function gets all songs withing a folder. or the file it's self (recursive)
fn get_songs(file_or_path: &Path) -> Vec<PathBuf> {
    if file_or_path.is_dir() {
        // if it is a folder we need to get all songs within said folder... recursively
        // create a array to hold all songs within this folder.
        let mut q = Vec::new();
        // now we iterate over all files. if it was able to read the folder.
        if let Ok(entries) = fs::read_dir(file_or_path) {
            // flatten the directory into a entry
            for entry in entries.flatten() {
                if entry.path().exists() {
                    q.extend(get_songs(&entry.path()));
                }
            }
        }
        // sort alphabetically
        q.sort_by(|a, b| b.cmp(a));
        q
    } else {
        // the path specified is a single file
        #[cfg(debug_assertions)]
        println!("{:?}",file_or_path);
        // we get the extension.
        let ext = file_or_path.extension().unwrap_or(OsStr::new("")).to_str().unwrap();
        match ext {
            "m3u" => { //playlist format so we add each line to the list
                let contents = fs::read_to_string(file_or_path).unwrap_or_default();
                let mut lines: Vec<&str> = contents.lines().collect();
                lines.reverse(); // the iterator reverses it. so to keep it in order. we reverse the lines here.

                let mut final_songs = Vec::new(); // create a final of list of songs
                for l in lines {
                    final_songs.extend(get_songs(&PathBuf::from_str(l).unwrap()));
                }
                final_songs
            }
            _ => vec![file_or_path.into()], // it is not a playlist so we just pass the file directly
        }
    }
}

/// updates playback state information via sovlaki
fn update_playback(state: &mut Status) {
    // get the handle's position in seconds
    let pos = state.handle.as_ref().map_or(0 as f64, |h| h.position());
    let duration = Some(MediaPosition(
        Duration::from_secs_f64(
            pos
        )
    ));// turn it into a position
    if state.paused { // if it is paused we set it as paused
        let _ = state.controls.set_playback(souvlaki::MediaPlayback::Paused { progress: duration });
    } else { // else we set it as playing.
        let _ = state.controls.set_playback(
            souvlaki::MediaPlayback::Playing { 
                progress: duration
            }
        );
    }
}

fn main() {
    let args = Args::parse(); // parse args
    let _ = MOD_FORMATS.set(get_supported_extensions().split(';').map(|x| x.to_string()).collect()); // init the MOD_FORMATS

    // souvlaki stuff... I just copied from the docs
    //#[cfg(not(target_os = "windows"))]
    let hwnd = None; 
    //#[cfg(target_os = "windows")]
    //let hwnd = {
    //    let event_loop = EventLoop::new();
    //    let window = WindowBuilder::new().build(&event_loop).unwrap();
    //    use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
    //    let handle: WindowsHandle = match window.raw_window_handle() {
    //        RawWindowHandle::Win32(h) => h,
    //        _ => unreachable!(),
    //    };
    //    Some(handle.hwnd)
    //};

    // dbus config so it shows up.
    let config = PlatformConfig {
        dbus_name: "redacted_music_player",
        display_name: "[Redacted]'s MusicBox",
        hwnd,
    };


    // init media controlls
    let mut controls = MediaControls::new(config).unwrap();
    // setup the event handler for all the media commands.
    controls
        .attach(|event| {
            //media button handler
            let mut state = GLOBAL_STATE.get().unwrap().lock().unwrap(); //lock the state so we can change it
            match event {
                MediaControlEvent::Next => state.play_next_song(),//skipping song
                MediaControlEvent::Pause => {
                    state.paused = true; //pause it
                    let _ = state.handle.as_mut().map(|h| h.pause(Tween::default()));
                },
                MediaControlEvent::Play => {
                    state.paused = false; //unpause it
                    let _ = state.handle.as_mut().map(|h| h.resume(Tween::default()));
                },
                MediaControlEvent::Toggle => {
                    let rg = state.paused;//are we paused?
                    if let Some(handle) = state.handle.as_mut() { let _ = if rg {
                            handle.resume(Tween::default())//unpause 
                        } else {
                            handle.pause(Tween::default())//pause
                        }; }
                    state.paused = !rg;
                },
                MediaControlEvent::Quit | MediaControlEvent::Stop => {exit(0)}, //quit the program
                MediaControlEvent::Previous => {state.do_the_previous_one()} //go back 1 song
                MediaControlEvent::SetPosition(pos) => { //seek to specific point in song
                    state.handle.as_mut().map(|h| h.seek_to(pos.0.as_secs_f64()));
                }
                MediaControlEvent::Seek(dir) => { //seed by a specified direction 10 seconds
                    state.handle.as_mut().map(|h| h.seek_by(match dir {
                        SeekDirection::Forward => 10.0,
                        SeekDirection::Backward => -10.0
                    }));
                }
                MediaControlEvent::SeekBy(dir, dur) => { //seeks by a specified number of seconds foward/bacl
                    state.handle.as_mut().map(|h| h.seek_by(
                        match dir {
                            SeekDirection::Forward => 1.0,
                            SeekDirection::Backward => -1.0
                        } * dur.as_secs_f64()
                    ));
                }
                x => println!("Event not yet implemented {:?}",x) //catch all for other un-implemented buttons (I have not found any)
            }
            update_playback(&mut state);
        })
        .unwrap();
    
    // Update the media metadata to a default.
    controls
        .set_metadata(MediaMetadata {
            title: Some("Walksanator Music Player"),
            artist: Some("Walksanator"),
            album: Some("Various Programs"),
            ..Default::default()
        })
        .unwrap();
    
    let manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default()).unwrap();

    #[cfg(debug_assertions)]
    println!("creating GLOBAL_STATE"); // setup the global state with all the instances created above.
    GLOBAL_STATE.set(Mutex::new(Status {
        paused: false,
        controls, 
        manager,
        upcoming: VecDeque::new(), 
        lookback: VecDeque::with_capacity(32),
        handle: None
    })).unwrap();
  
    loop {
        let mut state = GLOBAL_STATE.get().unwrap().lock().unwrap(); // wait to lock the global state (thread safe waiting for ownership)
        let stopped = !state.handle.as_ref().map_or(true, |x| x.state() == PlaybackState::Playing || x.state() == PlaybackState::Paused);
        // stopped is something dumb and I forgot why it works anymore. but it does so we dont question it
        if
            state.upcoming.is_empty() && stopped
            
        {
            // the queue is empty and no audio is playing. let us refill it or exit the program
            if args.looping {
                state.handle = None;
            } else {
                break
            }
        }
        // is the queue is empty and there is no currently playing audio
        // refill queue
        if state.upcoming.is_empty() && state.handle.is_none() {
            println!("filling queue.");
            let mut queue = vec![];
            for path in &args.files {
                queue.append(&mut get_songs(path));
            }
            queue.dedup(); // remove duplicate songs... (note: may remove this later)
            queue.reverse();
            if args.shuffle {
                print!("Shuffling...");
                queue.shuffle(&mut thread_rng());
                println!(" Done!");
            }
            state.upcoming.append(&mut queue.into());
            #[cfg(debug_assertions)]
            println!("upcoming {:?}",state.upcoming)
        }
        if stopped || state.handle.is_none() {
            println!("playing");
            state.play_next_song();
        } else {
            update_playback(&mut state);
        }
        drop(state); // release the lock before we sleep so other threads have 100ms to access it before we lock it again
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
