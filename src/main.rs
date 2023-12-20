
use std::{sync::{Mutex, OnceLock}, collections::VecDeque, fmt, fs, path::{Path, PathBuf}, ffi::OsStr, process::{exit, Command, Stdio}, time::Duration};

use clap::{command, Parser};
use kira::{manager::{AudioManager, backend::DefaultBackend, AudioManagerSettings}, sound::{PlaybackState, static_sound::{StaticSoundData, StaticSoundHandle, StaticSoundSettings}}, tween::Tween};
use openmpt::info::get_supported_extensions;
use souvlaki::{PlatformConfig, MediaControls, MediaMetadata, MediaControlEvent, MediaPosition, SeekDirection};
use rand::thread_rng;
use rand::seq::SliceRandom;


fn quoted(tgt: &str) -> Vec<String> {
    let mut res = vec![];
    let mut capture = false;
    let mut buf = vec![];
    for ch in tgt.chars() {
        if ch == '"' {
            if capture {
                let str: String = buf.iter().collect();
                res.push(str);
            } else {
                buf.clear()
            }
            capture = !capture;
        } else {
            buf.push(ch)
        }
    };
    res
}

struct Status {
    paused: bool,
    controls: MediaControls,
    manager: AudioManager,
    upcoming: VecDeque<PathBuf>,
    lookback: VecDeque<PathBuf>,
    handle: Option<StaticSoundHandle>
}

impl fmt::Debug for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Status").field("upcoming", &self.upcoming).field("lookback", &self.lookback).finish()
    }
}

impl Status {
    fn stopit(&mut self) {
        if self.handle.is_some() {
            self.handle.as_mut().unwrap().stop(Tween::default()).unwrap();
        }
    }
    fn play_next_song(&mut self) {
        self.stopit();
        println!("playing next song");
        #[cfg(debug_assertions)]
        println!("before {:?}",self.upcoming);
        let upcoming = if let Some(upcoming) = self.upcoming.pop_front() {
            upcoming
        } else {
            let _ = self.handle.as_mut().unwrap().stop(Tween::default());
            return;
        };
        #[cfg(debug_assertions)]
        println!("after {:?}",self.upcoming);
        self.push_song_to_lookback(upcoming.clone());
        let path = Path::new(&upcoming);
        #[cfg(debug_assertions)]
        println!("path {:?}",path);
        if !path.exists() {return}
        let ext = path.extension().unwrap_or(OsStr::new("")).to_str().unwrap_or("");
        #[cfg(debug_assertions)]
        println!("extension {}",ext);
        #[allow(unused_mut)]
        let mut meta = MediaMetadata {
            title: path.file_name().unwrap().to_str(),
            ..Default::default()
        };
        let sound = match ext {
            "wav" | "mp3" => {
                StaticSoundData::from_file(path, StaticSoundSettings::default()).unwrap()
            }
            x if MOD_FORMATS.get().unwrap().contains(&x.to_string()) => {
                let mut cmd = Command::new("openmpt123");
                cmd.args([path.to_str().unwrap(),"-o","/tmp/openmpt_convert.wav", "--force"]);
                cmd.stdout(Stdio::null());
                #[cfg(debug_assertions)]
                println!("{:?}",cmd);
                let _ = cmd.spawn().unwrap().wait();
                StaticSoundData::from_file("/tmp/openmpt_convert.wav", StaticSoundSettings::default()).unwrap()
                //let mut file = File::open(path).unwrap();
                //let module = Module::create(&mut file, Logger::None, &[]).unwrap();
                //StreamingSoundData::from_decoder(ModDecoder::new(module), StreamingSoundSettings::default())
            }
            _ => panic!("unsupported format '{}' file {}",ext,path.to_str().unwrap_or("failed to unwrap"))
        };
        meta.duration = Some(sound.duration());
        let hand = self.manager.play(sound).unwrap();
        self.handle = Some(hand);
        let _ = self.controls.set_metadata(meta);
        let _ = self.controls.set_playback(souvlaki::MediaPlayback::Playing { progress: None });
        println!("song is playing");
        
    }
    fn push_song_to_lookback(&mut self, song: PathBuf) {
        if self.lookback.len() == self.lookback.capacity() {
            let _ = self.lookback.pop_back(); //we know it is at capacity. and we are voiding it anyways.
        }
        self.lookback.push_front(song)
    }
    fn do_the_previous_one(&mut self) {
        if let Some(song) = self.lookback.pop_front() {
            self.upcoming.push_front(song);
        }
        if let Some(song) = self.lookback.pop_front() {
            self.upcoming.push_front(song);
        }
        self.play_next_song();
    }
}

#[derive(Parser, Debug)]
#[command(author = "walksanator", version = "v1", about = "command line music player", long_about = None)]
struct Args {
    #[arg(short, long, help = "sets whether or not to shuffle the music list")]
    shuffle: bool,

    #[arg(short, long, help = "sets looping of the music when all songs have been played")]
    looping: bool,
    
    #[arg(required(true))]
    files: Vec<PathBuf>,
}

static GLOBAL_STATE: OnceLock<Mutex<Status>> = OnceLock::new();
static MOD_FORMATS: OnceLock<Vec<String>> = OnceLock::new();

fn get_songs(file_or_path: &Path) -> Vec<PathBuf> {
    if file_or_path.is_dir() {
        let mut q = Vec::new();
        if let Ok(entries) = fs::read_dir(file_or_path) {
            for entry in entries.flatten() {
                if entry.path().exists() {
                    q.extend(get_songs(&entry.path()));
                }
            }
        }

        q.sort_by(|a, b| b.cmp(a));
        q
    } else {
        #[cfg(debug_assertions)]
        println!("{:?}",file_or_path);
        let ext = file_or_path.extension().unwrap_or(OsStr::new("")).to_str().unwrap();
        match ext {
            "m3u" => {
                let contents = fs::read_to_string(file_or_path).unwrap_or_default();
                let mut lines: Vec<&str> = contents.lines().collect();
                lines.reverse();

                let mut final_songs = Vec::new();
                for l in lines {
                    let p = Path::new(l);
                    if p.extension().unwrap().to_str().unwrap() == "m3u" {
                        let s = get_songs(p);
                        final_songs.extend(s);
                    } else {
                        final_songs.push(p.into());
                    }
                }
                final_songs
            }
            _ => vec![file_or_path.into()],
        }
    }
}

fn update_playback(state: &mut Status) {
    let pos = state.handle.as_ref().map_or(0 as f64, |h| h.position());
    let duration = Some(MediaPosition(
        Duration::from_secs_f64(
            pos
        )
    ));
    if state.paused {
        let _ = state.controls.set_playback(souvlaki::MediaPlayback::Paused { progress: duration });
    } else {
        let _ = state.controls.set_playback(
            souvlaki::MediaPlayback::Playing { 
                progress: duration
            }
        );
    }
}

fn main() {
    let args = Args::parse();
    let _ = MOD_FORMATS.set(get_supported_extensions().split(';').map(|x| x.to_string()).collect());
    #[cfg(debug_assertions)]
    println!("{:?}",args.files);
    #[cfg(not(target_os = "windows"))]
    let hwnd = None;
    #[cfg(target_os = "windows")]
    let hwnd = {
        use raw_window_handle::windows::WindowsHandle;

        let handle: WindowsHandle = unimplemented!();
        Some(handle.hwnd)
    };

    let config = PlatformConfig {
        dbus_name: "walksanator_music_player",
        display_name: "Walksanator's MusicBox",
        hwnd,
    };

    let mut controls = MediaControls::new(config).unwrap();
    controls
        .attach(|event| {
            let mut state = GLOBAL_STATE.get().unwrap().lock().unwrap();
            match event {
                MediaControlEvent::Next => state.play_next_song(),
                MediaControlEvent::Pause => {
                    state.paused = true;
                    let _ = state.handle.as_mut().map(|h| h.pause(Tween::default()));
                },
                MediaControlEvent::Play => {
                    state.paused = false;
                    let _ = state.handle.as_mut().map(|h| h.resume(Tween::default()));
                },
                MediaControlEvent::Toggle => {
                    let rg = state.paused;
                    state.handle.as_mut().map(|handle| {
                        let _ = if rg {
                            handle.resume(Tween::default())
                        } else {
                            handle.pause(Tween::default())
                        };
                    });
                    state.paused = !rg;
                },
                MediaControlEvent::Quit | MediaControlEvent::Stop => {exit(0)},
                MediaControlEvent::Previous => {state.do_the_previous_one()}
                MediaControlEvent::SetPosition(pos) => {
                    state.handle.as_mut().map(|h| h.seek_to(pos.0.as_secs_f64()));
                }
                MediaControlEvent::Seek(dir) => {
                    state.handle.as_mut().map(|h| h.seek_by(match dir {
                        SeekDirection::Forward => 10.0,
                        SeekDirection::Backward => -10.0
                    }));
                }
                MediaControlEvent::SeekBy(dir, dur) => {
                    state.handle.as_mut().map(|h| h.seek_by(
                        match dir {
                            SeekDirection::Forward => 1.0,
                            SeekDirection::Backward => -1.0
                        } * dur.as_secs_f64()
                    ));
                }
                x => println!("Event not yet implemented {:?}",x)
            }
            update_playback(&mut state);
        })
        .unwrap();

    // Update the media metadata.
    controls
        .set_metadata(MediaMetadata {
            title: Some("Walksanator Music Player"),
            artist: Some("Walksanator"),
            album: Some("Various Programs"),
            ..Default::default()
        })
        .unwrap();
    
    let manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default()).unwrap();


    GLOBAL_STATE.set(Mutex::new(Status {
        paused: false,
        controls, 
        manager,
        upcoming: VecDeque::new(), 
        lookback: VecDeque::with_capacity(32),
        handle: None
    })).unwrap();
  
    loop {
        let mut state = GLOBAL_STATE.get().unwrap().lock().unwrap();
        let stopped = !state.handle.as_ref().map_or(false, |x| x.state() == PlaybackState::Playing || x.state() == PlaybackState::Paused);
        if
            state.upcoming.is_empty() && stopped
            
        {
            if args.looping {
                state.handle = None;
            } else {
                break
            }
        }
        if state.upcoming.is_empty() && state.handle.is_none()  {
            #[cfg(debug_assertions)]
            println!("upcoming queue is empty");
            let mut queue = vec![];
            for path in &args.files {
                queue.append(&mut get_songs(path));
            }
            queue.dedup();
            if args.shuffle {
                queue.shuffle(&mut thread_rng());
            }
            state.upcoming.append(&mut queue.into());
            #[cfg(debug_assertions)]
            println!("upcoming {:?}",state.upcoming)
        }
        if stopped {
            println!("finished");
            state.play_next_song();
        } else {
            update_playback(&mut state);
        }
        drop(state);
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
