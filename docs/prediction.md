# Local prediction (speculative echo)

Status: **stock-aligned Diff path** (default `adaptive`)  
Related: [Netcatty #2121](https://github.com/binaricat/Netcatty/issues/2121)

## Architecture (must not change)

```text
HostBytes → apply_ansi → host_fb → Confirm → Overlay → Diff(last_shown) → PTY
Keystroke → Predictor → same Diff path
```

Never dual-write raw predicted glyphs beside HostBytes.  
Never require terminfo / Cygwin / system mosh.

## Alignment matrix

| Concern | Stock C++ | mosh-go | MoshCatty |
|---------|-----------|---------|-----------|
| Model | Framebuffer | Framebuffer | Framebuffer |
| Paint | new_frame | Diff | Diff |
| Confirm | cull + epochs | Confirm(fb) | Confirm + frame Pending |
| Printable | insert shift | pending | pending + host-row insert + mid shift |
| Backspace | row shift | Reset | undo / pending shift / host-row BS |
| Overwrite mode | CLI flag | n/a | `MOSH_PREDICTION_OVERWRITE` |
| L/R arrows | CSI C/D | none | CSI + SS3 |
| CR | tentative + row | n/a | tentative + row (no scroll) |
| Tentative epochs | hide until proven | n/a | hide epoch > confirmed |
| Frame Pending | late_ack | n/a | acked vs expiration_sent |
| Adaptive | send_interval 30/20 | n/a | send_interval≈SRTT/2 ∈[20,250] |
| Flagging | 80/50 ms | always under | 80/50 ms |
| Glitch | 250ms / 5s + 150ms repair | 500ms expire | same + no empty latch |
| Row change | prove anew | n/a | become_tentative |
| Renditions | match left | n/a | inherit left Attr |
| CorrectNoCredit | blank/noop | n/a | space/noop/unknown |
| unknown cells | last col / tails | n/a | shift tails |
| Cursor only | ConditionalCursorMove | n/a | cursor_exp_sent + confirm |
| Host model | full VT | minimal | CUP/SGR/EL/ED/ICH/DCH/IL/DL/scroll |
| Bulk paste | reset >100 | always | reset >100 |

## Env

| Variable | Values |
|----------|--------|
| `MOSH_PREDICTION_DISPLAY` | `adaptive` (default) / `always` / `never` |
| `MOSH_PREDICTION_OVERWRITE` | `yes`/`true`/`1` → overwrite instead of insert |

## Explicitly NOT implemented (protect Netcatty advantages)

| Rejected | Why |
|----------|-----|
| System / Cygwin mosh-client | Breaks pure single-binary Windows path |
| terminfo | Same |
| Full VTE-scale emulator | HostBytes+Diff sandwich is the product fit under node-pty |
| Forced alt-screen / exclusive TTY | Conflicts with `MOSH_NO_TERM_INIT` + xterm.js primary buffer |
| Scroll history prediction | Stock deferred; high garble risk |
| Up/down arrow prediction | Stock does not either |
| Notification / title chrome | Not Diff-path echo; Netcatty has own UI |
| Dual-write PTY echo | #2121 class bug |

## Modules

- `framebuffer.rs` — cells + Diff  
- `ansi_apply.rs` — HostBytes → host_fb  
- `prediction.rs` + `prediction_tests.rs` — Predictor + DisplayPipeline  
- `mosh_client.rs` — frames + send_interval wiring  
