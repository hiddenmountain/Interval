# 15. Error Conditions

The following conditions must produce a compile error. Compilation halts on the first error.

|Condition|Error|
|---|---|
|`steps` mismatch between declaration and body|`pattern <n>: declared steps=N but body has M lines`|
|Tie at pattern start with no prior context|`pattern <n> step 1: tie with no prior note`|
|`follow=` references undefined harmony block|`track <n>: harmony block '<x>' not defined`|
|`type=drums` combined with `follow=`|`track <n>: drum tracks cannot follow a harmony block`|
|`play:` and `steps:` both present|`track <n>: cannot have both play: and steps:`|
|Neither `play:` nor `steps:` present|`track <n>: must have either play: or steps:`|
|Pattern composition with mismatched `unit`|`pattern expression: cannot compose patterns with different units`|
|`ch=` outside range 1–16|`track <n>: ch must be 1–16`|
|`vel=` outside range 1–127|`<context>: vel must be 1–127`|
|`gate=` outside range 0–1|`<context>: gate must be 0.0–1.0`|
|`inv=N` exceeds chord tone count|`<context>: inversion N exceeds chord tone count`|
|Bar beats don't sum to time signature|`harmony <n> bar N: beat assignments sum to M, expected T`|
|Undefined pattern in `play:`|`track <n>: pattern '<x>' not defined`|
|`interleave(b)` with different step counts|`interleave: pattern step counts must match`|
|Forward reference|`pattern '<x>': forward references not permitted`|
|`section:` bar numbers not increasing|`harmony <n>: section bar numbers must be strictly increasing`|
|`mode=` on `@harmony`|`mode= is not valid on @harmony — use @scale`|
|Transform pipeline in wrong order|`transform pipeline: <x> must come before <y>`|
|`$chord` without harmony context|`track '<n>': $chord requires follow= or a single inferrable harmony block`|
|`%n` without harmony context|`track '<n>': %n requires follow= or a single inferrable harmony block`|
|`start=0`|`start= must be a positive integer (1-indexed)`|
|Multiple harmony blocks without names|`MultipleHarmonyBlocksRequireNames: all @harmony blocks must be named when more than one exists`|
|`@tempo` directive|`@tempo was removed in v0.5 — use @bpm block or inline form instead`|
|`\|` used as transform pipe operator|`DeprecatedPipeOperator: use -> instead of \| for the transform pipe operator`|
|`\|` used as variant pool separator|`DeprecatedVariantPipe: use {a,b,c} instead of {a\|b\|c} for variant pools`|
|`$_` used as current chord token|`DeprecatedCurrentChord: use $chord instead of $_`|
|Bare Roman numeral in non-heptatonic mode|`BareRomanNumeralInNonHeptatonicMode: quality cannot be inferred for degree <X> in <mode>`|
|`[prob:N]` with N outside 0.0–1.0|`prob annotation: value must be 0.0–1.0`|
|`[glide:N]` with N outside 0.0–1.0|`glide annotation: value must be 0.0–1.0`|
|`%n` with accidental|`chord ordinal %n does not support accidentals`|
|`ChordOrdinalWithoutHarmony`|`track '<n>': %n requires harmony context`|

**Warnings (compilation continues):**

|Condition|Warning|
|---|---|
|`^n` degree token with no harmony context|`track <n>: degree token with no follow; defaulting to @scale`|
|Note clamped to MIDI range|`track <n> step N: note clamped to 0–127`|
|`play=true` without `ch=`|`harmony <n>: play=true requires ch=`|
|No `@scale` declared|`no @scale declared — defaulting to C major`|
|`section:` directive inside `@harmony`|`` `section:` inside @harmony is deprecated in v0.5 — use `@scale` timeline form instead ``|
|`[prob:0.0]` annotation|`track <n> step N: probability annotation of 0.0 — step can never play`|
|`[glide]` annotation on drum track|`track <n> step N: [glide] annotation ignored on drum track`|

---

[← Back to Table of Contents](00_toc.md)
