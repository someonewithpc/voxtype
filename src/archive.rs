//! Keep every dictation's audio and text.
//!
//! When `VOXTYPE_ARCHIVE_DIR` is set, each recording that reaches
//! transcription is written as `<dir>/YYYY-MM-DD/HHMMSS-mmm.wav` (16 kHz mono
//! 16-bit), with a `.json` sidecar that collects the raw transcript, the
//! post-processed text and what happened to it. The audio is written before
//! transcription starts, so a crash or a failed paste still leaves it behind.
//!
//! When `VOXTYPE_FAILURE_COMMAND` is set, it runs as
//! `<command> <sidecar.json> <reason>` whenever a dictation does not end up
//! pasted, so the user can be told and offered a retry.

use serde_json::{json, Map, Value};
use std::path::PathBuf;

pub struct Entry {
    json_path: PathBuf,
    meta: Map<String, Value>,
}

impl Entry {
    /// Write the audio and an initial sidecar. Returns `None` when archiving
    /// is off or the files can't be written (logged).
    pub fn start(samples: &[f32], language: Option<String>) -> Option<Self> {
        let root = PathBuf::from(std::env::var_os("VOXTYPE_ARCHIVE_DIR")?);
        let now = chrono::Local::now();
        let dir = root.join(now.format("%Y-%m-%d").to_string());
        let stem = now.format("%H%M%S-%3f").to_string();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("archive: can't create {:?}: {}", dir, e);
            return None;
        }
        let wav_path = dir.join(format!("{stem}.wav"));
        if let Err(e) = write_wav(&wav_path, samples) {
            tracing::warn!("archive: can't write {:?}: {}", wav_path, e);
            return None;
        }

        let mut meta = Map::new();
        meta.insert("started".into(), json!(now.to_rfc3339()));
        meta.insert(
            "audio".into(),
            json!(wav_path.file_name().and_then(|n| n.to_str())),
        );
        meta.insert(
            "duration_secs".into(),
            json!(samples.len() as f64 / 16000.0),
        );
        meta.insert("language".into(), json!(language));
        let entry = Self {
            json_path: dir.join(format!("{stem}.json")),
            meta,
        };
        entry.save();
        Some(entry)
    }

    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        self.meta.insert(key.into(), value.into());
        self.save();
    }

    pub fn json_path(&self) -> &std::path::Path {
        &self.json_path
    }

    fn save(&self) {
        let tmp = self.json_path.with_extension("json.tmp");
        let body = match serde_json::to_vec_pretty(&self.meta) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("archive: can't encode sidecar: {}", e);
                return;
            }
        };
        if let Err(e) =
            std::fs::write(&tmp, body).and_then(|_| std::fs::rename(&tmp, &self.json_path))
        {
            tracing::warn!("archive: can't write {:?}: {}", self.json_path, e);
        }
    }
}

fn write_wav(path: &std::path::Path, samples: &[f32]) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &s in samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()
}

/// Record the outcome and, unless it was pasted, run the failure command.
pub fn finish(entry: Option<&mut Entry>, outcome: &str) {
    let json_path = entry.map(|e| {
        e.set("outcome", outcome);
        e.json_path().to_path_buf()
    });
    if outcome == "pasted" || outcome == "typed" {
        return;
    }
    notify_failure(json_path.as_deref(), outcome);
}

pub fn notify_failure(json_path: Option<&std::path::Path>, reason: &str) {
    let Some(cmd) = std::env::var_os("VOXTYPE_FAILURE_COMMAND") else {
        return;
    };
    let mut command = std::process::Command::new(cmd);
    command
        .arg(json_path.map(|p| p.as_os_str()).unwrap_or_default())
        .arg(reason)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    match command.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => tracing::warn!("failure command didn't start: {}", e),
    }
}
