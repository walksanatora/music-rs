use std::ffi::c_float;

use kira::{
    dsp::Frame,
    sound::{streaming::Decoder, FromFileError},
};
use openmpt::module::Module;

pub struct ModDecoder {
    frames: Vec<Frame>,
    pos: usize,
}

unsafe impl Send for ModDecoder {}

impl ModDecoder {
    pub fn new(mut module: Module) -> ModDecoder {
        println!("decoding duration: {}",module.get_duration_seconds());
        let mut frames = vec![];
        let mut bytes_poped = 1;
        let mut left: Vec<c_float> = Vec::with_capacity(22050);
        let mut right: Vec<c_float> = Vec::with_capacity(22050);
        while bytes_poped != 0 {
            bytes_poped = module.read_float_stereo(22050, &mut left, &mut right);
            println!("wrote {} bytes",bytes_poped);
            println!("{left:?} {right:?}");
            right.reverse();
            for val in left.iter() {
                println!("frame push");
                frames.push(
                    Frame {
                        left: val.clone(),
                        right: right.pop().unwrap() // they should hopefully be the same size.
                    }
                )
            }
            left.clear()
        };
        ModDecoder { frames, pos: 0 }
    }
}

impl Decoder for ModDecoder {
    type Error = FromFileError;

    fn sample_rate(&self) -> u32 {
        22050
    }

    fn num_frames(&self) -> usize {
        println!("num_frames called, no# of frames {}",self.frames.len());
        self.frames.len()
    }

    fn decode(&mut self) -> Result<Vec<kira::dsp::Frame>, Self::Error> {
        let frames = self.frames.as_slice()[self.pos..self.pos+11025].iter().map(|x| x.clone()).collect();
        self.pos += 11025;
        Ok(
            frames
        )
    }

    fn seek(&mut self, index: usize) -> Result<usize, Self::Error> {
        self.pos = index;
        Ok(self.pos)
    }
}
