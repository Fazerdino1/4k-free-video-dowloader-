//! Headless CLI runner shared by `vidgrab-cli` and the main `vidgrab` binary.
//!
//! Enables running `4k free video dowloader` on headless Linux servers, Raspberry Pi OS Lite,
//! Docker/Kubernetes containers, Alpine Linux (`musl`), and TTY-only environments without
//! initializing GTK or WebKitGTK.

use crate::config::{load_or_create_settings, AppPaths};
use crate::engine::{analyze_url, run_download};
use crate::media_tools::run_ffmpeg_tool;
use crate::platform::detect_platform_info;
use crate::updater::ensure_or_update_yt_dlp;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use vidgrab_shared::{
    AudioFormat, DownloadProgress, DownloadRequest, MediaToolAction, MediaType, VideoContainer,
};

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Prints CLI usage instructions to stdout or stderr.
pub fn print_usage(program_name: &str, to_stderr: bool) {
    let text = format!(
        "\
4k free video dowloader (vidgrab) v{APP_VERSION}
Moteur Open Source yt-dlp + ffmpeg pour Linux & Windows (Desktop & Serveur Headless)

USAGE :
  {program_name}                         Lance l'interface graphique (Desktop)
  {program_name} --help | -h             Affiche cette aide
  {program_name} --version | -V          Affiche la version
  {program_name} diag [--json]           Diagnostic OS / arch / libc / display / yt-dlp / ffmpeg
  {program_name} update [--force]        Installe ou met à jour yt-dlp selon l'architecture Linux
  {program_name} analyze <URL> [--json]  Analyse une vidéo ou playlist
  {program_name} download <URL> [OPTIONS]
      -o, --output <DIR>                 Dossier de destination
      -q, --quality <H|best>             Hauteur cible (2160, 1440, 1080, 720) ou 'best'
      --mkv | --mp4                      Conteneur vidéo (défaut : mp4)
      --audio <mp3|m4a|ogg|opus|flac|wav> Extraction audio uniquement
      --subs <fr,en,...>                 Télécharge et intègre les sous-titres
      --subs-only                        Télécharge uniquement les sous-titres
      --start <TS>                       Début de segment (SS, MM:SS, HH:MM:SS)
      --end <TS>                         Fin de segment (SS, MM:SS, HH:MM:SS)
      --proxy <URL>                      Proxy HTTP/HTTPS/SOCKS5 (ex: socks5h://127.0.0.1:9050)
  {program_name} ffmpeg <ACTION> <INPUT> [OPTIONS]
      remux                              Remux sans perte vers MP4 (+faststart)
      mp3                                Extraction audio MP3 320 kbps
      flac                               Extraction audio FLAC sans perte
      h264                               Compression H.264/AAC MP4
      trim --start <TS> --end <TS>       Découpe rapide sans ré-encodage
"
    );
    if to_stderr {
        eprint!("{text}");
    } else {
        print!("{text}");
    }
}

/// Evaluates process arguments and executes CLI mode if applicable.
///
/// Returns `Ok(None)` when GUI launch should proceed, or `Ok(Some(exit_code))` when handled as CLI.
pub async fn try_run_cli_from_args(args: &[String]) -> Option<i32> {
    if args.len() <= 1 {
        return None;
    }

    let bin = args.first().map(String::as_str).unwrap_or("vidgrab");
    let sub = args[1].as_str();
    let rest = &args[2..];

    match sub {
        "--gui" => None,
        "-h" | "--help" | "help" => {
            print_usage(bin, false);
            Some(0)
        }
        "-V" | "--version" | "version" => {
            println!("4k free video dowloader (vidgrab) {APP_VERSION}");
            Some(0)
        }
        "diag" => Some(cmd_diag(rest)),
        "update" => Some(cmd_update(rest).await),
        "analyze" => Some(cmd_analyze(rest).await),
        "download" => Some(cmd_download(rest).await),
        "ffmpeg" => Some(cmd_ffmpeg(rest).await),
        other => {
            eprintln!("Sous-commande ou option CLI inconnue : {other}\n");
            print_usage(bin, true);
            Some(2)
        }
    }
}

fn cmd_diag(args: &[String]) -> i32 {
    let as_json = args.iter().any(|a| a == "--json");
    let paths = AppPaths::initialize().unwrap_or_else(|_| AppPaths::resolve_without_creating());
    let settings = load_or_create_settings(&paths).unwrap_or_default();
    let info = detect_platform_info(&paths, &settings);

    if as_json {
        match serde_json::to_string_pretty(&info) {
            Ok(json) => {
                println!("{json}");
                return 0;
            }
            Err(err) => {
                eprintln!("Erreur JSON : {err}");
                return 1;
            }
        }
    }

    println!("=== 4k free video dowloader — Diagnostic Système ===");
    println!(
        "Distribution : {} (id={}, like={})",
        info.distro_name,
        info.distro_id,
        info.distro_like.join(",")
    );
    println!(
        "Architecture : {} | libc : {} | Init : {}",
        info.arch, info.libc, info.init_system
    );
    println!(
        "Affichage    : {} | Sandbox : {} | Immuable : {}",
        info.display_backend, info.sandbox, info.is_immutable
    );
    println!(
        "Environnement: NVIDIA={} | Qubes={} | Tails={}",
        info.has_nvidia, info.is_qubes, info.is_tails
    );
    if let Some(proxy) = &info.recommended_tor_proxy {
        println!("Proxy Tor    : {proxy}");
    }
    println!(
        "yt-dlp       : {} (asset cible: {})",
        info.yt_dlp_path
            .as_deref()
            .unwrap_or("non trouvé (exécutez `vidgrab update`)"),
        info.yt_dlp_asset
    );
    println!(
        "ffmpeg       : {}",
        info.ffmpeg_path
            .as_deref()
            .unwrap_or("non trouvé (installez ffmpeg)")
    );
    0
}

async fn cmd_update(args: &[String]) -> i32 {
    let force = args.iter().any(|a| a == "--force" || a == "-f");
    let paths = match AppPaths::initialize() {
        Ok(p) => p,
        Err(err) => {
            eprintln!("Erreur d'initialisation : {err}");
            return 1;
        }
    };
    println!("Vérification/mise à jour de yt-dlp...");
    match ensure_or_update_yt_dlp(&paths, force).await {
        Ok(msg) => {
            println!("{msg}");
            0
        }
        Err(err) => {
            eprintln!("Échec mise à jour yt-dlp : {err}");
            1
        }
    }
}

async fn cmd_analyze(args: &[String]) -> i32 {
    let mut url: Option<String> = None;
    let mut as_json = false;
    let mut proxy_override: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => as_json = true,
            "--proxy" => {
                i += 1;
                if let Some(val) = args.get(i) {
                    proxy_override = Some(val.clone());
                } else {
                    eprintln!("Option --proxy requiert une URL.");
                    return 2;
                }
            }
            val if !val.starts_with('-') && url.is_none() => url = Some(val.to_string()),
            other => {
                eprintln!("Argument inconnu pour analyze : {other}");
                return 2;
            }
        }
        i += 1;
    }

    let Some(target_url) = url else {
        eprintln!("Usage : vidgrab analyze <URL> [--json] [--proxy <URL>]");
        return 2;
    };

    let paths = match AppPaths::initialize() {
        Ok(p) => p,
        Err(err) => {
            eprintln!("Erreur chemins : {err}");
            return 1;
        }
    };
    let mut settings = load_or_create_settings(&paths).unwrap_or_default();
    if let Some(proxy_url) = proxy_override {
        settings.proxy.enabled = true;
        settings.proxy.url = proxy_url;
    }

    match analyze_url(&paths, &settings, &target_url, true).await {
        Ok(info) => {
            if as_json {
                match serde_json::to_string_pretty(&info) {
                    Ok(json) => println!("{json}"),
                    Err(err) => {
                        eprintln!("Erreur JSON : {err}");
                        return 1;
                    }
                }
            } else {
                println!("Titre    : {}", info.title);
                if let Some(up) = &info.uploader {
                    println!("Chaîne   : {up}");
                }
                if let Some(dur) = info.duration {
                    println!("Durée    : {dur:.0}s");
                }
                if info.is_playlist() {
                    println!("Playlist : {} entrée(s)", info.entries.len());
                } else {
                    println!("Formats  : {}", info.formats.len());
                }
            }
            0
        }
        Err(err) => {
            eprintln!("Erreur d'analyse : {err}");
            1
        }
    }
}

fn parse_audio_format(raw: &str) -> Option<AudioFormat> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "mp3" => Some(AudioFormat::Mp3),
        "m4a" => Some(AudioFormat::M4a),
        "ogg" | "vorbis" => Some(AudioFormat::Ogg),
        "opus" => Some(AudioFormat::Opus),
        "flac" => Some(AudioFormat::Flac),
        "wav" => Some(AudioFormat::Wav),
        _ => None,
    }
}

async fn cmd_download(args: &[String]) -> i32 {
    let paths = match AppPaths::initialize() {
        Ok(p) => p,
        Err(err) => {
            eprintln!("Erreur chemins : {err}");
            return 1;
        }
    };
    let mut settings = load_or_create_settings(&paths).unwrap_or_default();
    let mut req = DownloadRequest {
        output_dir: settings.video_output_dir.clone(),
        ..Default::default()
    };

    let mut url: Option<String> = None;
    let mut custom_output: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                i += 1;
                let Some(dir) = args.get(i) else {
                    eprintln!("Option --output requiert un dossier.");
                    return 2;
                };
                custom_output = Some(PathBuf::from(dir));
            }
            "-q" | "--quality" => {
                i += 1;
                let Some(q) = args.get(i) else {
                    eprintln!("Option --quality requiert une hauteur ou 'best'.");
                    return 2;
                };
                if q.eq_ignore_ascii_case("best") || q == "0" {
                    req.target_height = None;
                } else if let Ok(h) = q.trim_end_matches('p').parse::<u32>() {
                    req.target_height = Some(h);
                } else {
                    eprintln!("Qualité invalide : {q} (attendu: 2160, 1080, 720, best)");
                    return 2;
                }
            }
            "--mkv" => req.video_container = VideoContainer::Mkv,
            "--mp4" => req.video_container = VideoContainer::Mp4,
            "--audio" => {
                i += 1;
                let Some(fmt) = args.get(i).and_then(|s| parse_audio_format(s)) else {
                    eprintln!("Format audio invalide (mp3, m4a, ogg, opus, flac, wav).");
                    return 2;
                };
                req.media_type = MediaType::AudioOnly;
                req.audio_format = fmt;
            }
            "--subs" => {
                i += 1;
                let Some(langs) = args.get(i) else {
                    eprintln!("Option --subs requiert des codes de langue (ex: fr,en).");
                    return 2;
                };
                req.subtitle_languages = langs
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
                req.embed_subtitles = true;
            }
            "--subs-only" => req.media_type = MediaType::SubtitlesOnly,
            "--start" => {
                i += 1;
                let Some(ts) = args.get(i) else {
                    eprintln!("Option --start requiert un horodatage.");
                    return 2;
                };
                req.clip_start = Some(ts.clone());
            }
            "--end" => {
                i += 1;
                let Some(ts) = args.get(i) else {
                    eprintln!("Option --end requiert un horodatage.");
                    return 2;
                };
                req.clip_end = Some(ts.clone());
            }
            "--proxy" => {
                i += 1;
                let Some(proxy_url) = args.get(i) else {
                    eprintln!("Option --proxy requiert une URL.");
                    return 2;
                };
                settings.proxy.enabled = true;
                settings.proxy.url = proxy_url.clone();
            }
            val if !val.starts_with('-') && url.is_none() => url = Some(val.to_string()),
            other => {
                eprintln!("Option inconnue pour download : {other}");
                return 2;
            }
        }
        i += 1;
    }

    let Some(target_url) = url else {
        eprintln!("Usage : vidgrab download <URL> [-o DIR] [-q 1080] [--audio mp3]");
        return 2;
    };
    req.url = target_url;
    if let Some(dir) = custom_output {
        req.output_dir = dir;
    } else if req.media_type == MediaType::AudioOnly {
        req.output_dir = settings.audio_output_dir.clone();
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<DownloadProgress>();
    let logger = tokio::spawn(async move {
        let mut last_bucket: i32 = -1;
        while let Some(p) = rx.recv().await {
            let bucket = (p.percent / 10.0).floor() as i32;
            if bucket != last_bucket {
                last_bucket = bucket;
                println!("Progression : {:.1}% ({:?})", p.percent, p.status);
            }
        }
    });

    let res = run_download(&paths, &req, &settings, &[], tx, |_| {}).await;
    let _ = logger.await;

    match res {
        Ok(progress) => {
            if let Some(out) = progress.output_path {
                println!("Téléchargement terminé : {}", out.display());
            } else {
                println!("Téléchargement terminé dans : {}", req.output_dir.display());
            }
            0
        }
        Err(err) => {
            eprintln!("Échec du téléchargement : {err}");
            1
        }
    }
}

async fn cmd_ffmpeg(args: &[String]) -> i32 {
    let Some(action_raw) = args.first().map(String::as_str) else {
        eprintln!(
            "Usage : vidgrab ffmpeg <remux|mp3|flac|h264|trim> <INPUT> [--start TS --end TS]"
        );
        return 2;
    };

    let mut input_path: Option<PathBuf> = None;
    let mut start = String::new();
    let mut end = String::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--start" => {
                i += 1;
                let Some(val) = args.get(i) else {
                    eprintln!("Option --start requiert une valeur.");
                    return 2;
                };
                start = val.clone();
            }
            "--end" => {
                i += 1;
                let Some(val) = args.get(i) else {
                    eprintln!("Option --end requiert une valeur.");
                    return 2;
                };
                end = val.clone();
            }
            val if !val.starts_with('-') && input_path.is_none() => {
                input_path = Some(PathBuf::from(val));
            }
            other => {
                eprintln!("Argument inconnu pour ffmpeg : {other}");
                return 2;
            }
        }
        i += 1;
    }

    let Some(input) = input_path else {
        eprintln!("Chemin du fichier média source manquant.");
        return 2;
    };

    let action = match action_raw {
        "remux" => MediaToolAction::RemuxMp4,
        "mp3" => MediaToolAction::ExtractAudioMp3,
        "flac" => MediaToolAction::ExtractAudioFlac,
        "h264" => MediaToolAction::CompressH264,
        "trim" => MediaToolAction::TrimClip { start, end },
        other => {
            eprintln!("Action ffmpeg inconnue : {other} (remux|mp3|flac|h264|trim)");
            return 2;
        }
    };

    let paths = match AppPaths::initialize() {
        Ok(p) => p,
        Err(err) => {
            eprintln!("Erreur chemins : {err}");
            return 1;
        }
    };
    let settings = load_or_create_settings(&paths).unwrap_or_default();

    match run_ffmpeg_tool(&paths, &settings, Path::new(&input), &action).await {
        Ok(out) => {
            println!("Fichier généré : {}", out.display());
            0
        }
        Err(err) => {
            eprintln!("Erreur ffmpeg : {err}");
            1
        }
    }
}
