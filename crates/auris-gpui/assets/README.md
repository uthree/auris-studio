# Application icon

`auris-studio.png` is the source artwork: the Auris **A** formed by a flowing audio ribbon,
on a blue tile with transparent outer padding. Keep that padding and alpha when exporting.

The native formats are checked in so ordinary builds need no image-conversion tools:

- `auris-studio.ico`: 32-bit RGBA images at 16, 20, 24, 32, 40, 48, 64, 128 and 256 pixels.
  `windows.rc` embeds it as resource 1, which gpui loads for its Windows window class.
- `auris-studio.icns`: images from 16 through 1024 pixels, including Retina representations.
  The macOS release workflow copies it into `Contents/Resources`; `Info.plist` names it.

When changing the source artwork, regenerate both native formats with high-quality downsampling
and inspect the 16- and 32-pixel images on light and dark backgrounds.
