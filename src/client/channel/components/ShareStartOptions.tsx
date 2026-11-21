import { Chip } from "../../components/living/primitives";
import { useCopy } from "../../ui/copy";
import { QUALITY_PROFILES, qualityResolutionEqual, type QualityProfile, type QualityProfileId } from "../../media/quality";

export const SHARE_RESOLUTIONS = ["1080p", "1440p"] as const;
export const SHARE_FRAMERATES = [30, 60] as const;
export type ShareResolution = (typeof SHARE_RESOLUTIONS)[number];
export type ShareFramerate = (typeof SHARE_FRAMERATES)[number];

export function shareStartProfileId(resolution: ShareResolution, framerate: ShareFramerate): QualityProfileId {
  return `${resolution}${framerate}` as QualityProfileId;
}

/** Map any stored profile onto the two sizes and two frame rates this dialog asks. */
export function shareStartChoice(profile: QualityProfile): { resolution: ShareResolution; framerate: ShareFramerate } {
  return {
    resolution: profile.resolution.height >= 1440 ? "1440p" : "1080p",
    framerate: profile.maxFramerate >= 60 ? 60 : 30,
  };
}

/** Source, size, frame rate and source audio are asked once before a share starts. */
export function ShareStartOptions({ value, disabled, onChange }: {
  value: QualityProfile;
  disabled: boolean;
  onChange(profile: QualityProfile): void;
}) {
  const { t } = useCopy();
  const { resolution, framerate } = shareStartChoice(value);
  const select = (nextResolution: ShareResolution, nextFramerate: ShareFramerate) => {
    const profile = QUALITY_PROFILES[shareStartProfileId(nextResolution, nextFramerate)];
    if (qualityResolutionEqual(profile.resolution, value.resolution) && profile.maxFramerate === value.maxFramerate) return;
    onChange({ ...profile, maxAudioBitrate: value.maxAudioBitrate });
  };
  const summary = `${value.resolution.width} × ${value.resolution.height} · ${value.maxFramerate} fps · ${(value.maxBitrate / 1_000_000).toFixed(0)} Mbps`;
  return <div className="c3-share-start-options">
    <div className="c3-share-start-row">
      <span className="c3-share-start-label">{t("host.quality")}</span>
      <div className="lr-row-group" role="group" aria-label={`${t("host.quality")} · 1080p / 2K`}>
        {SHARE_RESOLUTIONS.map(size => <Chip key={size} name="share-resolution" value={size}
          selected={resolution === size} disabled={disabled}
          title={size === "1080p" ? "1080p" : "2K"}
          onClick={() => select(size, framerate)}>{size === "1080p" ? "1080p" : "2K"}</Chip>)}
      </div>
      <div className="lr-row-group" role="group" aria-label={`${t("host.quality")} · 30 / 60 fps`}>
        {SHARE_FRAMERATES.map(rate => <Chip key={rate} name="share-framerate" value={String(rate)}
          selected={framerate === rate} disabled={disabled}
          title={`${rate} fps`} onClick={() => select(resolution, rate)}>{rate} fps</Chip>)}
      </div>
    </div>
    <p className="c3-share-start-summary">{summary}</p>
  </div>;
}
