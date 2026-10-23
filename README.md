# Flecha

An Arc-style browser for developers, built on a Chromium fork. Arc's interface ideas are rebuilt from scratch; no Arc code is used.

## Why

Arc's vertical sidebar, Spaces and Peek are hard to give up, and Arc is no longer where the work is happening. Flecha is a replacement that keeps those ideas and adds tools for the people who build websites.

It is a Chromium 158 fork rather than an Electron app because Google blocks sign-in in Electron. Flecha is real Chromium, so Google sign-in, Chrome Web Store extensions and the rest of the web platform work as they do in Chrome. Flecha started as an Electron app; that version is gone from the repo (it's in the git history before the move to `chromium/`).

