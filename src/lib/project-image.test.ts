import { describe, it, expect } from "vitest";
import { faviconUrlForDomain, resolveImageUrl } from "./project-image";

describe("faviconUrlForDomain", () => {
  it("builds a size-specific URL and safely encodes cache-bust values", () => {
    expect(faviconUrlForDomain("github.com", 32, "fresh value")).toBe(
      "https://www.google.com/s2/favicons?domain=github.com&sz=32&v=fresh%20value",
    );
  });
});

describe("resolveImageUrl", () => {
  it("returns empty for blank input", () => {
    expect(resolveImageUrl("")).toEqual({
      url: "",
      isFavicon: false,
      domain: null,
    });
    expect(resolveImageUrl("   ")).toEqual({
      url: "",
      isFavicon: false,
      domain: null,
    });
  });

  it("passes data URLs through untouched", () => {
    const data = "data:image/png;base64,AAAA";
    expect(resolveImageUrl(data)).toEqual({
      url: data,
      isFavicon: false,
      domain: null,
    });
  });

  it("passes direct image URLs through by extension", () => {
    const png = "https://example.com/logo.png";
    expect(resolveImageUrl(png)).toEqual({
      url: png,
      isFavicon: false,
      domain: null,
    });
    // Extension with a trailing query string is still recognised.
    const ico = "https://example.com/fav.ico?x=1";
    expect(resolveImageUrl(ico).isFavicon).toBe(false);
    expect(resolveImageUrl(ico).url).toBe(ico);
    // …and so is one with a trailing fragment.
    const frag = "https://example.com/logo.png#section";
    expect(resolveImageUrl(frag).isFavicon).toBe(false);
    expect(resolveImageUrl(frag).url).toBe(frag);
  });

  it("derives a favicon URL from a bare domain", () => {
    const r = resolveImageUrl("codemux.com");
    expect(r.isFavicon).toBe(true);
    expect(r.domain).toBe("codemux.com");
    expect(r.url).toBe(
      "https://www.google.com/s2/favicons?domain=codemux.com&sz=128",
    );
  });

  it("strips a leading www and derives the favicon", () => {
    const r = resolveImageUrl("https://www.codemux.com/some/path");
    expect(r.isFavicon).toBe(true);
    expect(r.domain).toBe("codemux.com");
  });

  it("appends the cache-bust token to derived favicon URLs", () => {
    const a = resolveImageUrl("codemux.com", "123");
    expect(a.url).toBe(
      "https://www.google.com/s2/favicons?domain=codemux.com&sz=128&v=123",
    );

    // A different token yields a different URL, so the WebView re-fetches
    // instead of serving the previously cached favicon.
    const b = resolveImageUrl("codemux.com", "456");
    expect(b.url).not.toBe(a.url);
  });

  it("does not append a cache-bust token to direct or data URLs", () => {
    const png = "https://example.com/logo.png";
    expect(resolveImageUrl(png, "123").url).toBe(png);

    const data = "data:image/png;base64,AAAA";
    expect(resolveImageUrl(data, "123").url).toBe(data);
  });

  it("ignores an empty/zero cache-bust token", () => {
    const base =
      "https://www.google.com/s2/favicons?domain=codemux.com&sz=128";
    expect(resolveImageUrl("codemux.com", "").url).toBe(base);
    expect(resolveImageUrl("codemux.com", 0).url).toBe(base);
    expect(resolveImageUrl("codemux.com", null).url).toBe(base);
  });

  it("falls back to passthrough for unparseable input", () => {
    const r = resolveImageUrl("not a url");
    expect(r.isFavicon).toBe(false);
    expect(r.domain).toBe(null);
    expect(r.url).toBe("not a url");
  });
});
