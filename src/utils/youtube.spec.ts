import { describe, expect, it } from "vitest";
import { extractMetaTitleContent } from "./youtube.js";

describe("extractMetaTitleContent", () => {
	it("should extract the content value from a valid meta tag", () => {
		const html = `<meta name="title" content="Drum &amp; Bass Non-Stop Liquid - To Chill / Relax To 24/7">`;
		const result = extractMetaTitleContent(html);
		expect(result).toEqual({
			isLive: true,
			title: "Drum & Bass Non-Stop Liquid - To Chill / Relax To 24/7",
		});
	});

	it('should return null if the meta tag does not have the name="title"', () => {
		const html = `<meta name="description" content="Some description">`;
		const result = extractMetaTitleContent(html);
		expect(result).toEqual({ isLive: false });
	});

	it("should return null if the meta tag does not have the content attribute", () => {
		const html = `<meta name="title">`;
		const result = extractMetaTitleContent(html);
		expect(result).toEqual({ isLive: false });
	});

	it("should return null if the meta tag is not present", () => {
		const html = "<div>Some other content</div>";
		const result = extractMetaTitleContent(html);
		expect(result).toEqual({ isLive: false });
	});

	it("should extract the content value when there are multiple meta tags", () => {
		const html = `
      <meta name="description" content="Some description">
      <meta name="title" content="Drum &amp; Bass Non-Stop Liquid - To Chill / Relax To 24/7">
      <meta name="keywords" content="music, chill, relax">
    `;
		const result = extractMetaTitleContent(html);
		expect(result).toEqual({
			isLive: true,
			title: "Drum & Bass Non-Stop Liquid - To Chill / Relax To 24/7",
		});
	});

	it("should extract the content value with escaped double quotes", () => {
		const html = `<meta name="title" content="A &quot;quoted&quot; title">`;
		const result = extractMetaTitleContent(html);
		expect(result).toEqual({
			isLive: true,
			title: 'A "quoted" title',
		});
	});

	it("should handle meta tags with double quotes in content correctly", () => {
		const html = `<meta name="title" content="This is a &quot;test&quot; title">`;
		const result = extractMetaTitleContent(html);
		expect(result).toEqual({
			isLive: true,
			title: 'This is a "test" title',
		});
	});

	it("should extract content value with mixed special characters", () => {
		const html = `<meta name="title" content="Test with &quot;quotes&quot; &amp; special characters">`;
		const result = extractMetaTitleContent(html);
		expect(result).toEqual({
			isLive: true,
			title: 'Test with "quotes" & special characters',
		});
	});
});
