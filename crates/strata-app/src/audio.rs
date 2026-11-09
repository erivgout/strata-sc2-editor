//! Plays sounds packed into the map, so they can be checked before they are
//! used (Assets panel). Desktop only; the output device opens on first use.

use std::io::Cursor;

#[derive(Default)]
pub struct Player {
    out: Option<(rodio::OutputStream, rodio::Sink)>,
    /// Map file key of the sound playing (or last played).
    current: Option<String>,
}

impl Player {
    /// Play `bytes` (MP3, Ogg Vorbis or WAV), stopping whatever was playing.
    pub fn play(&mut self, key: &str, bytes: Vec<u8>) -> Result<(), String> {
        self.stop();
        if self.out.is_none() {
            let mut stream = rodio::OutputStreamBuilder::open_default_stream()
                .map_err(|e| format!("Strata couldn't open an audio device ({e})."))?;
            stream.log_on_drop(false);
            let sink = rodio::Sink::connect_new(stream.mixer());
            self.out = Some((stream, sink));
        }
        let decoder = rodio::Decoder::new(Cursor::new(bytes))
            .map_err(|e| format!("Strata can't play this file ({e}). StarCraft II plays MP3, Ogg Vorbis and WAV."))?;
        if let Some((_, sink)) = &self.out {
            sink.append(decoder);
            sink.play();
        }
        self.current = Some(key.to_owned());
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some((_, sink)) = &self.out {
            sink.stop();
        }
        self.current = None;
    }

    /// The map file playing right now, if any.
    pub fn playing(&self) -> Option<&str> {
        let busy = self.out.as_ref().is_some_and(|(_, s)| !s.empty());
        if busy { self.current.as_deref() } else { None }
    }
}
