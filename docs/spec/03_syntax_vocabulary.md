# 3. Syntax Vocabulary

The complete set of reserved syntax characters and their roles. Where a character has multiple roles, context determines interpretation unambiguously.

|Token|Role|Example|
|---|---|---|
|`^`|Degree token prefix (scale-absolute)|`^1`, `^b7`, `^#11`|
|`%n`|Chord ordinal token|`%1`, `%3`, `%5/4`|
|`+`|Simultaneous notes (step level)|`^1+^3+^5`|
|`.`|Rest|`.`|
|`(...)`|Subdivision bracket|`(^1 ^3 ^5)`|
|`[...]`|Step annotation bracket|`^1[vel:110 shift:-3%]`|
|`{a,b,c}`|Variant pool|`{^1,^b7,^5}`|
|`~`|Tie / hold|`^1 ~ ~ ~`|
|`*`|Pattern repetition (hard boundary)|`pattern * 4`|
|`*~`|Pattern repetition (soft tie boundary)|`pattern *~ 4`|
|`>>`|Pattern concatenation (hard boundary)|`verse >> chorus`|
|`~>>`|Pattern concatenation (soft tie boundary)|`verse ~>> bridge`|
|`$`|Chord symbol prefix (in step lines)|`$Cmaj7`, `$G7b9`|
|`$chord`|Current harmony chord (in step lines)|`$chord`, `$chord[vel:80]`|
|`\|`|Bar separator in `@harmony` blocks, `@bpm` timeline entries, `@ts` timeline entries, `@scale` timeline entries|`Cmaj7 \| Am7` / `120 \| 140`|
|`->`|Transform pipe operator (in `play:` / `@pattern` expressions, lowest precedence)|`a -> reverse`|
|`:`|Key-value separator in annotations|`vel:90`|
|`=`|Key-value separator in directives|`ch=1`|
|`@`|Block declaration prefix / per-ref rate|`@track` / `theme@2.0`|
|`-->`|Ramp operator|`expr:40->88`, `120->160`|
|`()`|Expression grouping|`(a -> reverse) >> b`|
|`//`|Line comment|`// this is ignored`|

**Reserved-role rules:**

- `->` is the only transform pipe operator. Using `|` in an expression context (outside a harmony bar grid or timeline) produces a `DeprecatedPipeOperator` error.
- `,` is the only variant pool separator. Using `|` inside `{}` produces a `DeprecatedVariantPipe` error.
- `$chord` is the only current chord token. Using `$_` produces a `DeprecatedCurrentChord` error.

### 3.1 Whitespace

Whitespace (spaces, tabs) is insignificant within step lines except as a separator between step tokens. Newlines are significant: each step occupies exactly one line. Blank lines are ignored. Indentation is permitted and encouraged but not required.

### 3.2 Comments

`//` begins a line comment. Everything from `//` to the end of the line is ignored by the compiler.

```
@track bass ch=2 prog=32  // upright bass
```

Block comments are not supported.

---

[← Back to Table of Contents](00_toc.md)
