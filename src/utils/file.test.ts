import { describe, it, expect } from "vitest";
import { isYamlFile, isHtmlFile, isImageFile, imageMimeType, isFilePath } from "./file";

describe("isYamlFile", () => {
  it("detects .yaml files", () => {
    expect(isYamlFile("config.yaml")).toBe(true);
  });

  it("detects .yml files", () => {
    expect(isYamlFile("config.yml")).toBe(true);
  });

  it("is case insensitive", () => {
    expect(isYamlFile("Config.YAML")).toBe(true);
    expect(isYamlFile("Config.YML")).toBe(true);
  });

  it("rejects non-yaml files", () => {
    expect(isYamlFile("config.json")).toBe(false);
    expect(isYamlFile("config.txt")).toBe(false);
    expect(isYamlFile("yaml")).toBe(false);
  });
});

describe("isHtmlFile", () => {
  it("detects .html files", () => {
    expect(isHtmlFile("page.html")).toBe(true);
  });

  it("detects .htm files", () => {
    expect(isHtmlFile("page.htm")).toBe(true);
  });

  it("is case insensitive", () => {
    expect(isHtmlFile("page.HTML")).toBe(true);
  });

  it("rejects non-html files", () => {
    expect(isHtmlFile("page.txt")).toBe(false);
    expect(isHtmlFile("html")).toBe(false);
  });
});

describe("isImageFile", () => {
  it("detects common image formats", () => {
    expect(isImageFile("photo.png")).toBe(true);
    expect(isImageFile("photo.jpg")).toBe(true);
    expect(isImageFile("photo.jpeg")).toBe(true);
    expect(isImageFile("photo.gif")).toBe(true);
    expect(isImageFile("photo.webp")).toBe(true);
    expect(isImageFile("photo.bmp")).toBe(true);
    expect(isImageFile("photo.ico")).toBe(true);
    expect(isImageFile("photo.svg")).toBe(true);
  });

  it("is case insensitive", () => {
    expect(isImageFile("photo.PNG")).toBe(true);
    expect(isImageFile("photo.JPG")).toBe(true);
  });

  it("rejects non-image files", () => {
    expect(isImageFile("doc.pdf")).toBe(false);
    expect(isImageFile("script.js")).toBe(false);
    expect(isImageFile("png")).toBe(false);
  });
});

describe("imageMimeType", () => {
  it("returns correct mime types", () => {
    expect(imageMimeType("photo.png")).toBe("image/png");
    expect(imageMimeType("photo.jpg")).toBe("image/jpeg");
    expect(imageMimeType("photo.jpeg")).toBe("image/jpeg");
    expect(imageMimeType("photo.gif")).toBe("image/gif");
    expect(imageMimeType("photo.webp")).toBe("image/webp");
    expect(imageMimeType("photo.bmp")).toBe("image/bmp");
    expect(imageMimeType("photo.ico")).toBe("image/x-icon");
    expect(imageMimeType("photo.svg")).toBe("image/svg+xml");
  });

  it("returns octet-stream for unknown extensions", () => {
    expect(imageMimeType("file.xyz")).toBe("application/octet-stream");
  });
});

describe("isFilePath", () => {
  it("detects file paths", () => {
    expect(isFilePath("src/App.tsx")).toBe(true);
    expect(isFilePath("README.md")).toBe(true);
    expect(isFilePath("src/utils/format.ts")).toBe(true);
    expect(isFilePath("Cargo.toml")).toBe(true);
    expect(isFilePath(".gitignore")).toBe(false); // starts with dot
  });

  it("handles paths with hyphens and underscores", () => {
    expect(isFilePath("my-component_v2.tsx")).toBe(true);
    expect(isFilePath("src/my-dir/file_name.rs")).toBe(true);
  });

  it("rejects non-file strings", () => {
    expect(isFilePath("hello world")).toBe(false);
    expect(isFilePath("not a file")).toBe(false);
    expect(isFilePath("")).toBe(false);
    expect(isFilePath("no-extension")).toBe(false);
  });

  it("trims whitespace", () => {
    expect(isFilePath("  file.ts  ")).toBe(true);
  });
});
