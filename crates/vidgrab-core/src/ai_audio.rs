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

fn venv_bin_path(venv_dir: &Path, name: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        venv_dir.join("Scripts").join(format!("{name}.exe"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        venv_dir.join("bin").join(name)
    }
}

/// Checks whether Demucs is installed inside the local `demucs-venv` directory.
pub fn is_demucs_installed(paths: &AppPaths) -> bool {
    venv_bin_path(&paths.demucs_venv_dir, "demucs").exists()
}

/// Detects available hardware acceleration (`"cuda"`, `"rocm"`, or `"cpu"`).
pub async fn detect_compute_device() -> &'static str {
    let mut nv_cmd = Command::new("nvidia-smi");
    nv_cmd.stdout(Stdio::null()).stderr(Stdio::null());
    crate::configure_background_command(&mut nv_cmd);
    if let Ok(out) = nv_cmd.status().await {
        if out.success() {
            return "cuda";
        }
    }
    let mut rocm_cmd = Command::new("rocminfo");
    rocm_cmd.stdout(Stdio::null()).stderr(Stdio::null());
    crate::configure_background_command(&mut rocm_cmd);
    if let Ok(out) = rocm_cmd.status().await {
        if out.success() {
            return "cuda"; // PyTorch ROCm uses the `cuda` device string in Demucs `-d cuda`
        }
    }
    "cpu"
}

/// Installs Demucs on-demand into the local `demucs-venv` Python environment.
pub async fn install_demucs_venv(paths: &AppPaths) -> CoreResult<String> {
    let venv_dir = &paths.demucs_venv_dir;
    info!("Creating Python virtual environment at {:?}", venv_dir);

    let python_bin = if cfg!(target_os = "windows") {
        if which::which("python").is_ok() {
            "python"
        } else {
            "python3"
        }
    } else {
        "python3"
    };

    let mut venv_cmd = Command::new(python_bin);
    venv_cmd.arg("-m").arg("venv").arg(venv_dir);
    crate::configure_background_command(&mut venv_cmd);
    let status = venv_cmd.status().await.map_err(|e| {
        CoreError::AiProcessingFailed(format!("Failed to launch {python_bin} -m venv: {e}"))
    })?;

    if !status.success() {
        return Err(CoreError::AiProcessingFailed(
            "python -m venv failed. Make sure Python 3 is installed.".to_string(),
        ));
    }

    let pip_bin = venv_bin_path(venv_dir, "pip");
    info!("Installing demucs inside virtual environment...");
    let mut pip_cmd = Command::new(&pip_bin);
    pip_cmd
        .args(["install", "--upgrade", "pip", "demucs"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::configure_background_command(&mut pip_cmd);
    let output = pip_cmd
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
/// Processes audio with Demucs/FFmpeg and reports stage progress via callback.
pub async fn process_audio_ai(
    paths: &AppPaths,
    settings: &Settings,
    input_file: &Path,
    op: AiAudioOperation,
) -> CoreResult<PathBuf> {
    process_audio_ai_with_progress(paths, settings, input_file, op, |_| {}).await
}

/// Processes audio with Demucs/FFmpeg and reports `(stage, percent, message)` updates.
pub async fn process_audio_ai_with_progress<F>(
    paths: &AppPaths,
    settings: &Settings,
    input_file: &Path,
    op: AiAudioOperation,
    on_progress: F,
) -> CoreResult<PathBuf>
where
    F: Fn(vidgrab_shared::AiAudioProgress) + Send + Sync,
{
    if !input_file.exists() {
        return Err(CoreError::AiProcessingFailed(format!(
            "Input file does not exist: {}",
            input_file.display()
        )));
    }

    on_progress(vidgrab_shared::AiAudioProgress {
        stage: "prepare".to_string(),
        percent: 10.0,
        message: "Initialisation du traitement audio IA...".to_string(),
    });

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
        let demucs_bin = venv_bin_path(&paths.demucs_venv_dir, "demucs");
        let device = detect_compute_device().await;
        on_progress(vidgrab_shared::AiAudioProgress {
            stage: "demucs".to_string(),
            percent: 35.0,
            message: format!("Séparation Demucs en cours ({device})..."),
        });
        let out_root = parent.join("demucs_out");

        let mut demucs_cmd = Command::new(&demucs_bin);
        demucs_cmd
            .args([
                "--two-stems=vocals",
                "-d",
                device,
                "-o",
                &out_root.to_string_lossy(),
                &input_file.to_string_lossy(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::configure_background_command(&mut demucs_cmd);
        let demucs_status = demucs_cmd.status().await;

        if let Ok(st) = demucs_status {
            if st.success() {
                let target_stem = match op {
                    AiAudioOperation::RemoveVocals => "no_vocals.wav",
                    _ => "vocals.wav",
                };
                let candidate = out_root.join("htdemucs").join(stem).join(target_stem);
                if candidate.exists() {
                    if op == AiAudioOperation::IsolateVoiceNoise {
                        on_progress(vidgrab_shared::AiAudioProgress {
                            stage: "ffmpeg".to_string(),
                            percent: 75.0,
                            message: "Nettoyage DSP FFmpeg des voix...".to_string(),
                        });
                        // Chain ffmpeg afftdn on top of the Demucs vocal stem
                        let final_out = parent.join(format!("{stem}_{suffix}.wav"));
                        run_ffmpeg_filter(
                            &ffmpeg,
                            &candidate,
                            &final_out,
                            build_ffmpeg_audio_filter(op),
                        )
                        .await?;
                        on_progress(vidgrab_shared::AiAudioProgress {
                            stage: "done".to_string(),
                            percent: 100.0,
                            message: "Traitement audio IA terminé.".to_string(),
                        });
                        return Ok(final_out);
                    }
                    on_progress(vidgrab_shared::AiAudioProgress {
                        stage: "done".to_string(),
                        percent: 100.0,
                        message: "Traitement Demucs terminé.".to_string(),
                    });
                    return Ok(candidate);
                }
            }
        }
    }

    // Fallback or Echo/Reverb reduction via ffmpeg filters
    on_progress(vidgrab_shared::AiAudioProgress {
        stage: "ffmpeg".to_string(),
        percent: 55.0,
        message: "Application du filtre audio FFmpeg DSP...".to_string(),
    });
    let output_file = parent.join(format!("{stem}_{suffix}.mp3"));
    let filter = build_ffmpeg_audio_filter(op);
    run_ffmpeg_filter(&ffmpeg, input_file, &output_file, filter).await?;
    on_progress(vidgrab_shared::AiAudioProgress {
        stage: "done".to_string(),
        percent: 100.0,
        message: "Traitement audio terminé.".to_string(),
    });
    Ok(output_file)
}

async fn run_ffmpeg_filter(
    ffmpeg: &Path,
    input: &Path,
    output: &Path,
    filter: &str,
) -> CoreResult<()> {
    let args = vec![
        "-y".to_string(),
        "-i".to_string(),
        input.to_string_lossy().into_owned(),
        "-vn".to_string(),
        "-af".to_string(),
        filter.to_string(),
        output.to_string_lossy().into_owned(),
    ];

    if let Some(bridge) = crate::mobile_bridge::get() {
        bridge.run_ffmpeg(args).await?;
        let _ = bridge
            .scan_media_file(output.to_string_lossy().into_owned())
            .await;
        return Ok(());
    }

    let mut cmd = Command::new(ffmpeg);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::configure_background_command(&mut cmd);
    let out = cmd.output().await?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(CoreError::AiProcessingFailed(format!(
            "ffmpeg filter failed: {stderr}"
        )));
    }
    Ok(())
}
