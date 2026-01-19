import { describe, expect, it } from "vitest";
import { LiveStatus } from "./index.js";
import { extractLiveStatus } from "./youtube.js";

describe.skip("extractLiveStatus", () => {
  const wrapWithYtData = (html: string) =>
    `<script>var ytInitialPlayerResponse = {};</script>${html}`;

  it("should extract the content value from a valid meta tag", () => {
    const html = wrapWithYtData(
      `"isLive":true<meta name="title" content="Drum &amp; Bass Non-Stop Liquid - To Chill / Relax To 24/7">`,
    );
    const result = extractLiveStatus(html);
    expect(result).toEqual({
      status: LiveStatus.Live,
      title: "Drum & Bass Non-Stop Liquid - To Chill / Relax To 24/7",
      viewerCount: undefined,
    });
  });

  it("should return offline when not live", () => {
    const html = wrapWithYtData(`<meta name="description" content="Some description">`);
    const result = extractLiveStatus(html);
    expect(result).toEqual({ status: LiveStatus.Offline });
  });

  it("should return unknown when ytInitialPlayerResponse is missing", () => {
    const html = "<div>Some other content</div>";
    const result = extractLiveStatus(html);
    expect(result).toEqual({
      status: LiveStatus.Unknown,
      error: "Response missing expected YouTube data structure",
    });
  });

  it("should return unknown when live but title extraction fails", () => {
    const html = wrapWithYtData(`"isLive":true`);
    const result = extractLiveStatus(html);
    expect(result).toEqual({
      status: LiveStatus.Unknown,
      error: "Live detected but failed to extract title",
    });
  });

  it("should extract the content value when there are multiple meta tags", () => {
    const html = wrapWithYtData(`
			"isLive":true
			<meta name="description" content="Some description">
			<meta name="title" content="Drum &amp; Bass Non-Stop Liquid - To Chill / Relax To 24/7">
			<meta name="keywords" content="music, chill, relax">
		`);
    const result = extractLiveStatus(html);
    expect(result).toEqual({
      status: LiveStatus.Live,
      title: "Drum & Bass Non-Stop Liquid - To Chill / Relax To 24/7",
      viewerCount: undefined,
    });
  });

  it("should extract the content value with escaped double quotes", () => {
    const html = wrapWithYtData(
      `"isLive":true<meta name="title" content="A &quot;quoted&quot; title">`,
    );
    const result = extractLiveStatus(html);
    expect(result).toEqual({
      status: LiveStatus.Live,
      title: 'A "quoted" title',
      viewerCount: undefined,
    });
  });

  it("should handle meta tags with double quotes in content correctly", () => {
    const html = wrapWithYtData(
      `"isLive":true<meta name="title" content="This is a &quot;test&quot; title">`,
    );
    const result = extractLiveStatus(html);
    expect(result).toEqual({
      status: LiveStatus.Live,
      title: 'This is a "test" title',
      viewerCount: undefined,
    });
  });

  it("should extract content value with mixed special characters", () => {
    const html = wrapWithYtData(
      `"isLive":true<meta name="title" content="Test with &quot;quotes&quot; &amp; special characters">`,
    );
    const result = extractLiveStatus(html);
    expect(result).toEqual({
      status: LiveStatus.Live,
      title: 'Test with "quotes" & special characters',
      viewerCount: undefined,
    });
  });
});
