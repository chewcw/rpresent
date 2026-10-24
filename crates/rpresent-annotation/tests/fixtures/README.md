# Test font fixture

`DejaVuSans.ttf` is DejaVu Sans, version 2.37, copied unmodified from the
Debian `fonts-dejavu-core` package (`/usr/share/fonts/truetype/dejavu/`).

- Upstream: https://dejavu-fonts.github.io/
- License: Bitstream Vera Fonts Copyright plus the DejaVu changes; see
  `DejaVuSans.LICENSE.txt` (the package copyright file).

The annotation text tests load this font explicitly with system font discovery
disabled so layout results do not depend on the host's installed fonts.
