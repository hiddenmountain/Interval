# 18. Known Limitations and Future Work

### Current Limitations

**No polyphonic aftertouch.** Channel aftertouch (`at`) is supported; per-note is not.

**No MIDI program changes mid-track.** Single program change at tick 0 per track.

**No sysex or NRPN.** Out of scope.

**No grace notes.** Use `shift` as workaround.

**Forward references not permitted.** By design.

**`section:` in `@harmony` deprecated but not removed.** Will produce a deprecation warning until a future version removes it entirely.

For the history of syntax and semantics changes between releases (including migration tables), see `CHANGELOG.md` at the repository root.

### Reserved for Future Versions

- **Independent parameter lanes (Matriceal)** — different parameter lanes with independent lengths, speeds, and directions per track
- **Arpoly (polymetric arpeggiator)** — programmable arpeggiator with independent loops
- **`reharmonize()` transform** — harmony context modification
- **`voice_lead()` transform** — generative voice-leading lines
- **User-defined transform functions**
- **Mid-track program changes**
- **Polyphonic aftertouch**
- **Sysex and NRPN**
- **Block comments**

---

[← Back to Table of Contents](00_toc.md)
