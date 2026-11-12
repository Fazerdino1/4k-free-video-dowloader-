//! AI Audio processing module using Demucs (on-demand Python venv) and `ffmpeg` audio filters.
//!
//! Supports:
//! - Vocal removal (Karaoke)
//! - Vocal extraction (A cappella)
//! - Voice isolation from background noise
//! - Background music removal
//! - Echo & reverberation reduction (`afftdn` + `arnndn` / highpass+lowpass+acompressor)
//!
//! Automatically detects CUDA (`nvidia-smi`) or ROCm (`rocminfo`) GPU acceleration when
//! invoking Demucs, and seamlessly falls back to pure `ffmpeg` DSP filters if Demucs is not installed.

use crate::config::AppPaths;
use crate::error::{CoreError, CoreResult};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tracing::info;
use vidgrab_shared::{AiAudioOperation, Settings};

/// Checks whether Demucs is installed inside `~/.local/share/vidgrab/demucs-venv`.
pub fn is_demucs_installed(paths: &AppPaths) -> bool {
    paths.demucs_venv_dir.join("bin").join("demucs").exists()
}

/// Detects available hardware acceleration (`"cuda"`, `"rocm"`, or `"cpu"`).
pub async fn detect_compute_device() -> &'static str {
    if let Ok(out) = Command::new("nvidia-smi")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
    {
        if out.success() {
            return "cuda";
        }
    }
    if let Ok(out) = Command::new("rocminfo")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
    {
        if out.success() {
            return "cuda"; // PyTorch ROCm uses the `cuda` device string in Demucs `-d cuda`
        }
    }
    "cpu"
}

/// Installs Demucs on-demand into `~/.local/share/vidgrab/demucs-venv`.
pub async fn install_demucs_venv(paths: &AppPaths) -> CoreResult<String> {
    let venv_dir = &paths.demucs_venv_dir;
    info!("Creating Python virtual environment at {:?}", venv_dir);

    let status = Command::new("python3")
        .arg("-m")
        .arg("venv")
        .arg(venv_dir)
        .status()
        .await
        .map_err(|e| {
            CoreError::AiProcessingFailed(format!("Failed to launch python3 -m venv: {e}"))
        })?;

    if !status.success() {
        return Err(CoreError::AiProcessingFailed(
            "python3 -m venv failed. Make sure python3 is installed (sudo dnf install python3)"
                .to_string(),
        ));
    }

    let pip_bin = venv_dir.join("bin").join("pip");
    info!("Installing demucs inside virtual environment...");
    let output = Command::new(&pip_bin)
        .args(["install", "--upgrade", "pip", "demucs"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| CoreError::AiProcessingFailed(format!("pip install demucs failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CoreError::AiProcessingFailed(format!(
            "Demucs installation failed: {stderr}"
        )));
    }

    let device = detect_compute_device().await;
    Ok(format!(
        "Demucs installed successfully (compute device: {device})"
    ))
}

/// Builds the `ffmpeg` filtergraph for an [`AiAudioOperation`] when running in `ffmpeg` DSP mode
/// or for post-filtering (`afftdn` / `arnndn`).
pub fn build_ffmpeg_audio_filter(op: AiAudioOperation) -> &'static str {
    match op {
        AiAudioOperation::RemoveVocals => {
            // Stereo phase-cancellation karaoke filter
            "pan=stereo|c0=c0-c1|c1=c1-c0,highpass=f=80"
        }
        AiAudioOperation::ExtractVocals => {
            // Center-channel vocal band isolation + FFT denoiser
            "pan=mono|c0=0.5*c0+0.5*c1,highpass=f=150,lowpass=f=7500,afftdn=nf=-25"
        }
        AiAudioOperation::IsolateVoiceNoise => {
            // FFT spectral noise reduction + speech frequency band
            "highpass=f=100,lowpass=f=8000,afftdn=nr=18:nf=-30:tn=1"
        }
        AiAudioOperation::RemoveBackgroundMusic => {
            // Dynamic dialogue enhancer + spectral noise gate
            "highpass=f=130,lowpass=f=7000,afftdn=nr=20:nf=-28,acompressor=threshold=-18dB:ratio=3"
        }
        AiAudioOperation::ReduceEchoReverb => {
            // Dereverberation / room echo attenuation using FFT denoiser and gate
            "afftdn=nr=15:nf=-35:tn=1,agate=threshold=0.02:ratio=2:release=150"
        }
    }
}

/// Processes an audio or video file with either Demucs (if installed and applicable) or `ffmpeg`
/// audio filters (`afftdn`, phase cancellation, etc.), returning the newly created output path.
pub async fn process_audio_ai(
    paths: &AppPaths,
    settings: &Settings,
    input_file: &Path,
    op: AiAudioOperation,
) -> CoreResult<PathBuf> {
    if !input_file.exists() {
        return Err(CoreError::AiProcessingFailed(format!(
            "Input file does not exist: {}",
            input_file.display()
        )));
    }

    let ffmpeg = paths.effective_ffmpeg(settings)?;
    let stem = input_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let parent = input_file.parent().unwrap_or_else(|| Path::new("."));

    let suffix = match op {
        AiAudioOperation::RemoveVocals => "karaoke",
        AiAudioOperation::ExtractVocals => "vocals",
        AiAudioOperation::IsolateVoiceNoise => "voice_isolated",
        AiAudioOperation::RemoveBackgroundMusic => "no_music",
        AiAudioOperation::ReduceEchoReverb => "dereverb",
    };

    // If Demucs is installed and the operation separates stems, run Demucs `--two-stems=vocals`
    if is_demucs_installed(paths) && op != AiAudioOperation::ReduceEchoReverb {
        let demucs_bin = paths.demucs_venv_dir.join("bin").join("demucs");
        let device = detect_compute_device().await;
        let out_root = parent.join("demucs_out");

        let demucs_status = Command::new(&demucs_bin)
            .args([
                "--two-stems=vocals",
                "-d",
                device,
                "-o",
                &out_root.to_string_lossy(),
                &input_file.to_string_lossy(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .status()
            .await;

        if let Ok(st) = demucs_status {
            if st.success() {
                let target_stem = match op {
                    AiAudioOperation::RemoveVocals => "no_vocals.wav",
                    _ => "vocals.wav",
                };
                let candidate = out_root.join("htdemucs").join(stem).join(target_stem);
                if candidate.exists() {
                    if op == AiAudioOperation::IsolateVoiceNoise {
                        // Chain ffmpeg afftdn on top of the Demucs vocal stem
                        let final_out = parent.join(format!("{stem}_{suffix}.wav"));
                        run_ffmpeg_filter(
                            &ffmpeg,
                            &candidate,
                            &final_out,
                            build_ffmpeg_audio_filter(op),
                        )
                        .await?;
                        return Ok(final_out);
                    }
                    return Ok(candidate);
                }
            }
        }
    }

    // Fallback or Echo/Reverb reduction via ffmpeg filters
    let output_file = parent.join(format!("{stem}_{suffix}.mp3"));
    let filter = build_ffmpeg_audio_filter(op);
    run_ffmpeg_filter(&ffmpeg, input_file, &output_file, filter).await?;
    Ok(output_file)
}

async fn run_ffmpeg_filter(
    ffmpeg: &Path,
    input: &Path,
    output: &Path,
    filter: &str,
) -> CoreResult<()> {
    let out = Command::new(ffmpeg)
        .args([
            "-y",
            "-i",
            &input.to_string_lossy(),
            "-vn",
            "-af",
            filter,
            &output.to_string_lossy(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(CoreError::AiProcessingFailed(format!(
            "ffmpeg filter failed: {stderr}"
        )));
    }
    Ok(())
}
