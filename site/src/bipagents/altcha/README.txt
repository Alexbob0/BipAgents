ALTCHA's widget for « Obtenir un code » (/code/), served from bipagents.com itself: no CDN, no request elsewhere.

Package: altcha 3.4.1 from npm (published 2026-10-19), MIT (LICENSE-altcha.txt, copied to the site with it).
Tarball:  https://registry.npmjs.org/altcha/-/altcha-3.4.1.tgz
          sha512-K+5Mm1kvdFb+MWr6zIttXXvpGWXTYKD/SxYgXG3TbeeVjEny9DgqyevcuXui4GLtx0roIqC2KYYhSnp5/Vdzbg==
          sha256 b57899afc29972f8623678f8a36b526802e0c4eaacf70224747ce0cc98d9afc8
Files, unchanged: dist/external/altcha.min.js and altcha.css (the build for a strict Content-Security-Policy: no
inline worker), dist/workers/pbkdf2.js (the PBKDF2/SHA-256 worker the page registers), LICENSE.txt.

The guichet makes and checks the challenges (Guichet repo, guichet-rs/src/altcha.rs, its protocol: widget 3.x).
Changing the version: take the same four files from the new tarball, check its licence, the guichet's tests
(altcha.rs: the widget's own derivation) and the page in a browser.
