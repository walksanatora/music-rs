mod moddecoder;
use std::{sync::{Mutex, OnceLock}, collections::VecDeque, fmt, fs::{self, File}, path::{Path, PathBuf}, ffi::OsStr, process::exit};

use clap::{command, Parser};
use kira::{manager::{AudioManager, backend::DefaultBackend, AudioManagerSettings}, sound::{streaming::{StreamingSoundData, StreamingSoundSettings, StreamingSoundHandle}, FromFileError}};
use openmpt::{info::get_supported_extensions, module::{Module, Logger}};
use souvlaki::{PlatformConfig, MediaControls, MediaMetadata, MediaControlEvent};
use rand::thread_rng;
use rand::seq::SliceRandom;

use crate::moddecoder::ModDecoder;

fn quoted(tgt: &String) -> Vec<String> {
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
    controls: MediaControls,
    manager: AudioManager,
    upcoming: VecDeque<String>,
    lookback: VecDeque<String>,
    handle: Option<StreamingSoundHandle<FromFileError>>
}

impl fmt::Debug for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Status").field("upcoming", &self.upcoming).field("lookback", &self.lookback).finish()
    }
}

impl Status {
    fn play_next_song(&mut self) {
        println!("playing next song");
        let upcoming = self.upcoming.pop_front().unwrap();
        self.push_song_to_lookback(upcoming.clone());
        let path = Path::new(&upcoming);
        println!("path {:?}",path);
        if !path.exists() {return}
        let ext = path.extension().unwrap_or(OsStr::new("")).to_str().unwrap_or("");
        println!("extension {}",ext);
        let sound = match ext {
            "wav" => {
                StreamingSoundData::from_file(path, StreamingSoundSettings::default()).unwrap()
            }
            x if MOD_FORMATS.get().unwrap().contains(&x.to_string()) => {
                let mut file = File::open(path).unwrap();
                let module = Module::create(&mut file, Logger::None, &[]).unwrap();
                StreamingSoundData::from_decoder(ModDecoder::new(module), StreamingSoundSettings::default())
            }
            _ => panic!("unsupported format '{}' file {}",ext,path.to_str().unwrap_or("failed to unwrap"))
        };
        self.handle = Some(self.manager.play(sound).unwrap());
        println!("song is playing");
        
    }
    fn push_song_to_lookback(&mut self, song: String) {
        if self.lookback.len() == self.lookback.capacity() {
            let _ = self.lookback.pop_back(); //we know it is at capacity. and we are voiding it anyways.
        }
        self.lookback.push_front(song)
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

fn get_songs(file_or_path: &Path) -> Vec<String> {
    if file_or_path.is_dir() {
        let mut q = Vec::new();
        if let Ok(entries) = fs::read_dir(&file_or_path) {
            for entry in entries.flatten() {
                if entry.path().exists() {
                    q.extend(get_songs(&entry.path()));
                }
            }
        }

        q.sort_by(|a, b| b.cmp(a));
        return q;
    } else {
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
                        let s = get_songs(&p);
                        final_songs.extend(s);
                    } else {
                        final_songs.push(String::from(l));
                    }
                }
                return final_songs;
            }
            _ => return vec![file_or_path.to_str().unwrap().to_string()],
        }
    }
}

fn main() {
    let args = Args::parse();
    let _ = MOD_FORMATS.set(get_supported_extensions().split(";").map(|x| x.to_string()).collect());
    
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
                // MediaControlEvent::Pause => {state.handle.as_mut().map(|h| h.guard().paused = true);},
                // MediaControlEvent::Play => {state.handle.as_mut().map(|h| h.guard().paused = false);},
                // MediaControlEvent::Toggle => {
                //     let mut rg = state.handle.as_mut().unwrap().guard();
                //     rg.paused = !rg.paused;
                // },
                MediaControlEvent::Quit | MediaControlEvent::Stop => {exit(0)},
                x => println!("Event not yet implemented {:?}",x)
            }
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
        controls, 
        manager,
        upcoming: VecDeque::new(), 
        lookback: VecDeque::with_capacity(32),
        handle: None
    })).unwrap();

    let mut once_bool = true; //so the loop runs once before letting looping take over
    
    loop {
        let mut state = GLOBAL_STATE.get().unwrap().lock().unwrap();
        if !(args.looping || once_bool) && !state.upcoming.is_empty() && state.manager.num_sounds()==0 {
            println!("exiting lp:{}\nonce: {}, upcoming: {}",args.looping,once_bool,state.upcoming.is_empty());
            break
        };
        once_bool=false;
        if state.upcoming.is_empty() {
            println!("upcoming queue is empty");
            let mut queue = vec![];
            for path in &args.files {
                queue.append(&mut get_songs(&path));
            }
            queue.dedup();
            if args.shuffle {
                queue.shuffle(&mut thread_rng());
            }
            state.upcoming.append(&mut queue.into());
            println!("upcoming {:?}",state.upcoming)
        }
        if state.manager.num_sounds() == 0 {
            println!("finished");
            state.play_next_song();
        } else {
            println!(
                "it is still playing"
            )    
        }
        drop(state);
        // sl.voice_count() > 0 {
        //     std::thread::sleep(std::time::Duration::from_millis(100));
        // }    
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
