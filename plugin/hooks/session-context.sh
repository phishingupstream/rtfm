#!/usr/bin/env bash
# SessionStart: expose local context, then refresh cached sources when due.
command -v rtfm >/dev/null 2>&1 || exit 0

rtfm status 2>/dev/null || true

if command -v timeout >/dev/null 2>&1; then
    if timeout --kill-after=1 8 rtfm update-if-due >/dev/null 2>&1; then
        exit 0
    fi
fi
printf '%s\n' 'RTFM source refresh did not complete; run rtfm update-if-due for details.' >&2
exit 0
