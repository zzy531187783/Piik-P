import { useEffect, useState } from "react";
import { Btn, Chip } from "../../components/living/primitives";
import { CaptureSourceCard } from "../../components/living/CaptureSourceCard";
import { useCopy } from "../../ui/copy";
import { Glyph } from "../../ui/icons";
import { nativeCaptureTargetKey } from "../../native/capture-selection";
import type { NativeCaptureTarget } from "../../native/wire";
import type { ChannelController } from "../types";
import type { QualityProfile } from "../../media/quality";
import "./channel-share-dialog.css";

type Resolution = "720" | "1080" | "original";
type Framerate = 15 | 30 | 60;

function shareProfile(resolution: Resolution, framerate: Framerate): QualityProfile {
  const base = resolution === "720"
    ? { width: 1280, height: 720, bitrate: 2_500_000 }
    : resolution === "1080"
      ? { width: 1920, height: 1080, bitrate: 5_000_000 }
      : { width: 3840, height: 2160, bitrate: 20_000_000 };
  const factor = framerate >= 60 ? 1.6 : framerate >= 30 ? 1 : 0.6;
  return {
    resolution: { width: base.width, height: base.height },
    maxFramerate: framerate,
    maxBitrate: Math.round(base.bitrate * factor),
    degradationPreference: "balanced",
    maxAudioBitrate: 128_000,
  };
}

function SourceCard({ target, selected, onPreview, onSelect }: {
  target: NativeCaptureTarget; selected: boolean;
  onPreview(target: NativeCaptureTarget): Promise<string | null>; onSelect(): void;
}) {
  const [preview, setPreview] = useState<string | null | undefined>(undefined);
  useEffect(() => {
    let active = true;
    void onPreview(target).then(value => { if (active) setPreview(value); }).catch(() => { if (active) setPreview(null); });
    return () => { active = false; };
  }, [target, onPreview]);
  return <CaptureSourceCard title={target.title} action={target.title}
    icon={target.kind === "picker" ? "share" : target.kind}
    preview={preview} nativeKey={nativeCaptureTargetKey(target)}
    selected={selected} onPreview={() => undefined} onSelect={onSelect} />;
}

/** One modal asks source, size, frame rate and source audio before a share starts. */
export function ChannelShareDialog({ channel, roomId, onClose }: {
  channel: ChannelController; roomId: string; onClose(): void;
}) {
  const { t } = useCopy();
  const [sources, setSources] = useState<Awaited<ReturnType<ChannelController["nativeSources"]>> | null>(null);
  const [tab, setTab] = useState<"window" | "display" | "browser">("window");
  const [selected, setSelected] = useState<NativeCaptureTarget | null>(null);
  const [resolution, setResolution] = useState<Resolution>("1080");
  const [framerate, setFramerate] = useState<Framerate>(30);
  const [audio, setAudio] = useState(false);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let active = true;
    void channel.nativeSources().then(value => { if (active) setSources(value); }).catch(() => undefined);
    return () => { active = false; };
  }, [channel.nativeSources]);
  const ready = sources;
  const targets = ready ? ready.sources.filter(target => target.kind === tab) : [];
  const audioSupported = !!ready && (tab === "display" ? ready.systemAudio : tab === "window" ? ready.processAudio : false);
  const canStart = tab === "browser" || selected !== null;
  const start = async () => {
    if (!canStart || busy) return;
    setBusy(true);
    try {
      channel.setQuality(shareProfile(resolution, framerate));
      if (tab === "browser") await channel.startScreen(roomId);
      else if (selected) await channel.startNativeScreen(roomId, selected, audio && audioSupported, undefined, false);
      onClose();
    } catch {
      setBusy(false);
    }
  };
  return <div className="lr-share-backdrop" onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}>
    <section className="lr-share-dialog" role="dialog" aria-modal="true" aria-label={t("screen.screenSharing")}>
      <header className="lr-share-dialog-head">
        <h2>{t("screen.screenSharing")}</h2>
        <button type="button" className="lr-btn is-dense is-quiet" aria-label={t("common.cancel")} onClick={onClose}>
          <Glyph name="x" size={18} />
        </button>
      </header>
      <div className="lr-share-dialog-body">
        <div className="lr-share-sources">
          <div className="lr-share-tabs" role="tablist">
            {(["window", "display", "browser"] as const).map(value => <button key={value} type="button" role="tab"
              aria-selected={tab === value} className={tab === value ? "is-selected" : undefined}
              onClick={() => { setTab(value); setSelected(null); }}>
              {t(value === "browser" ? "host.sourcePicker.tab.browser" : `host.sourcePicker.tab.${value}`)}
            </button>)}
          </div>
          <div className="lr-share-source-grid">
            {tab === "browser" ? <button type="button" className="lr-share-browser-card" onClick={() => undefined}>
              <Glyph name="globe" size={26} /><span>{t("host.sourcePicker.browser")}</span>
            </button> : targets.length === 0 ? <p className="lr-share-empty">{t("host.sourcePicker.empty")}</p>
              : targets.map(target => <SourceCard key={nativeCaptureTargetKey(target)} target={target}
                selected={!!selected && nativeCaptureTargetKey(selected) === nativeCaptureTargetKey(target)}
                onPreview={target => channel.nativePreview(target)} onSelect={() => setSelected(target)} />)}
          </div>
        </div>
        <aside className="lr-share-settings">
          <div className="lr-share-field">
            <span className="lr-share-label">{t("share.dialog.resolution")}</span>
            <div className="lr-row-group" role="group">
              <Chip selected={resolution === "720"} title="720P" onClick={() => setResolution("720")}>720P</Chip>
              <Chip selected={resolution === "1080"} title="1080P" onClick={() => setResolution("1080")}>1080P</Chip>
              <Chip selected={resolution === "original"} title={t("share.dialog.original")} onClick={() => setResolution("original")}>{t("share.dialog.original")}</Chip>
            </div>
          </div>
          <div className="lr-share-field">
            <span className="lr-share-label">{t("share.dialog.framerate")}</span>
            <div className="lr-row-group" role="group">
              {([15, 30, 60] as const).map(value => <Chip key={value} selected={framerate === value} title={`${value} fps`}
                onClick={() => setFramerate(value)}>{value} fps</Chip>)}
            </div>
          </div>
          <div className="lr-share-field">
            <span className="lr-share-label">{t("host.sourcePicker.systemAudio")}</span>
            <button type="button" className="lr-switch" role="switch" aria-checked={audio && audioSupported}
              aria-label={t("host.sourcePicker.systemAudio")} disabled={!audioSupported}
              onClick={() => setAudio(value => !value)} />
            <p className="lr-share-hint">{t("share.dialog.audioHint")}</p>
          </div>
          <Btn icon="cast" tone="primary" title="host.start" cap="host.start" size="compact"
            disabled={!canStart || busy} busy={busy} onClick={() => { void start(); }} />
        </aside>
      </div>
    </section>
  </div>;
}
