//! Modular v0.4.0 UI sections: live stats dashboard, visual media trim bar, and v4 settings.

/// Renders the compact live statistics & visual trim bar injected above the download list.
#[must_use]
pub fn render_v4_dashboard_section() -> &'static str {
    r#"
    <section id="v4-dashboard" class="queue-bar" style="margin-top:4px;gap:12px;flex-wrap:wrap;" aria-label="Tableau de bord v0.4">
        <div style="display:flex;gap:10px;align-items:center;flex-wrap:wrap;font-size:12px;">
            <span class="count-pill" id="stat-active">En cours : 0</span>
            <span class="count-pill" id="stat-done">Terminés : 0</span>
            <span class="count-pill" id="stat-failed">Échecs : 0</span>
        </div>
        <div style="display:flex;gap:6px;align-items:center;font-size:12px;">
            <label for="trim-file-input" style="opacity:0.8;">Trim visuel :</label>
            <input id="trim-file-input" type="file" accept="video/*,audio/*" style="max-width:170px;font-size:11px;" />
            <input id="trim-start-sec" type="number" min="0" value="0" style="width:62px;" title="Début (s)" />
            <span>→</span>
            <input id="trim-end-sec" type="number" min="0" value="30" style="width:62px;" title="Fin (s)" />
            <button id="btn-trim-preview" type="button" class="queue-btn">▶ Prévisualiser</button>
            <button id="btn-whisper-transcribe" type="button" class="queue-btn" title="Transcrire un fichier local en .srt avec Whisper">🎙️ Whisper</button>
        </div>
        <video id="trim-preview-player" controls style="display:none;width:100%;max-height:220px;border-radius:8px;margin-top:6px;background:#000;"></video>
    </section>
    "#
}

/// Renders the v0.4.0 settings controls (media integrity, post-process pipeline, domain profiles).
#[must_use]
pub fn render_v4_settings_section() -> &'static str {
    r#"
    <label class="check-field"><input id="set-verify-media" type="checkbox" /><span>Vérifier l'intégrité post-téléchargement (ffmpeg -v error)</span></label>
    <label class="check-field"><input id="set-pipe-loudnorm" type="checkbox" /><span>Pipeline : Normaliser l'audio (EBU R128 loudnorm)</span></label>
    <label class="check-field"><input id="set-pipe-silence" type="checkbox" /><span>Pipeline : Supprimer les silences</span></label>
    <label class="check-field"><input id="set-pipe-hw" type="checkbox" /><span>Pipeline : Ré-encodage matériel GPU (NVENC/VAAPI/QSV)</span></label>
    <label class="form-field" style="grid-column:1/-1;">
        <span>Profils par domaine (JSON : [{"domain":"soundcloud.com","media_type":"AudioOnly","audio_format":"Flac"}])</span>
        <textarea id="set-domain-profiles" rows="2" style="width:100%;font-family:monospace;font-size:12px;" placeholder="[]"></textarea>
    </label>
    "#
}
