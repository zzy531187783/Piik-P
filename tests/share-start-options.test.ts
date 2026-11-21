import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ShareStartOptions, shareStartChoice, shareStartProfileId } from "../src/client/channel/components/ShareStartOptions";
import { QUALITY_PROFILES } from "../src/client/media/quality";
import { setCopy } from "../src/client/ui/copy";

describe("one-step share quality", () => {
  it("maps the four size and frame-rate choices onto recommended profiles", () => {
    expect(shareStartProfileId("1080p", 30)).toBe("1080p30");
    expect(shareStartProfileId("1080p", 60)).toBe("1080p60");
    expect(shareStartProfileId("1440p", 30)).toBe("1440p30");
    expect(shareStartProfileId("1440p", 60)).toBe("1440p60");
    expect(shareStartChoice(QUALITY_PROFILES["1440p60"])).toEqual({ resolution: "1440p", framerate: 60 });
    expect(shareStartChoice(QUALITY_PROFILES["720p30"])).toEqual({ resolution: "1080p", framerate: 30 });
  });

  it("renders both sizes and both frame rates with the current choice selected", () => {
    setCopy({ lang: "en" });
    const html = renderToStaticMarkup(createElement(ShareStartOptions, {
      value: QUALITY_PROFILES["1440p60"], disabled: false, onChange() {},
    }));
    expect(html).toContain("1080p");
    expect(html).toContain("2K");
    expect(html).toContain("30 fps");
    expect(html).toContain("60 fps");
    expect(html.match(/aria-pressed="true"/g)).toHaveLength(2);
    expect(html).toContain("2560 × 1440 · 60 fps · 16 Mbps");
  });
});
