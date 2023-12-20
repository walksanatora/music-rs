#![allow(dead_code)]
use std::sync::{Arc, Mutex, MutexGuard};

use kittyaudio::{RendererHandle, Renderer, SoundHandle, Frame, Backend, cpal, Device, StreamSettings};
use openmpt::module::Module;

#[derive(Clone)]
pub enum AudioType {
    Module(Arc<Mutex<Module>>),
    Generic(SoundHandle),
    None
}

impl PartialEq for AudioType { // two audio types are considered equal if they are the same type but contents can differ
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Module(_), Self::Module(_)) => true,
            (Self::Generic(_), Self::Generic(_)) => true,
            (Self::None, Self::None) => true,
            _ => false,
        }
    }
}


#[derive(Clone)]
pub struct SingleRender {
    audio: AudioType,
    left: Vec<f32>,
    right: Vec<f32>,
    paused: bool
}

unsafe impl Send for SingleRender {}

impl Renderer for SingleRender {
    fn next_frame(&mut self, sample_rate: u32) -> kittyaudio::Frame {
        if self.paused {return Frame::ZERO}
        let mut frames_emptied = false;
        let frame = match &self.audio {
            AudioType::Module(modu) => {
                if self.left.is_empty() || self.right.is_empty() {
                    let mut module = modu.lock().unwrap();
                    let bytes = module.read_float_stereo(sample_rate as i32, &mut self.left, &mut self.right);
                    if bytes == 0 {frames_emptied = true}
                }
                Frame {
                    left: self.left.pop().unwrap(),
                    right: self.right.pop().unwrap()
                }
            }
            AudioType::Generic(handle) => {
                if let Some(frame) = handle.guard().next_frame(sample_rate) {
                    frame
                } else {
                    frames_emptied = true;
                    Frame::ZERO
                }
            }
            AudioType::None => {Frame::ZERO}
        };
        if frames_emptied {self.audio = AudioType::None};
        frame
    }
}

impl SingleRender {
    pub fn set_audio(&mut self, aud: AudioType) {
        self.left.clear();
        self.right.clear();
        self.audio = aud;
    }
    pub fn is_paused(&self) -> bool {self.paused}
    pub fn set_paused(&mut self,paused: bool) {self.paused = paused}
}

pub struct CustomMixer {
    pub render: RendererHandle<SingleRender>,
    pub backend: Arc<Mutex<Backend>>
}

impl CustomMixer {
    pub fn new() -> Self {
        Self {
            render: RendererHandle::new(
                SingleRender {
                    audio: AudioType::None, 
                    left: Vec::with_capacity(22040), 
                    right: Vec::with_capacity(22040),
                    paused: false
                }
            ),
            backend: Arc::new(Mutex::new(Backend::new())),
        }
    }
    #[inline(always)]
    pub fn backend(&self) -> MutexGuard<'_, Backend> {
        self.backend.lock().unwrap()
    }
    pub fn play_generic(&mut self, sound: impl Into<SoundHandle>) -> SoundHandle {
        let handle: SoundHandle = sound.into();
        self.render.guard()
            .set_audio(AudioType::Generic(handle.clone()));
        handle
    }
    pub fn play_tracker(&mut self, track: Module) -> Arc<Mutex<Module>> {
        let handle = Arc::from(Mutex::from(track));
        self.render.guard()
            .set_audio(AudioType::Module(handle.clone()));
        handle
    }
    #[inline]
    pub fn handle_errors(&mut self, err_fn: impl FnMut(cpal::StreamError)) {
        self.backend().handle_errors(err_fn);
    }
    #[inline]
    pub fn init(&self) {
        self.init_ex(Device::Default, StreamSettings::default());
    }
    pub fn init_ex(&self, device: Device, settings: StreamSettings) {
        let backend = self.backend.clone();
        let renderer = self.render.clone();
        std::thread::spawn(move || {
            // TODO: handle errors from `start_audio_thread`
            let _ = backend
                .lock().unwrap()
                .start_audio_thread(device, settings, renderer);
        });
    }
    pub fn wait(&self) {
        while self.render.guard().audio != AudioType::None  {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    #[inline]
    pub fn is_finished(&self) -> bool {
        self.render.guard().audio == AudioType::None
    }
    #[inline]
    pub fn next_frame(&self, sample_rate: u32) -> Frame {
        self.render.guard().next_frame(sample_rate)
    }
}

