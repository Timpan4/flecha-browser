#!/usr/bin/env bash
# Check 7, print, on scripts/print-test.html.
# OSR: Ctrl+P in the window must log "[print] start", and the `pdf` command must write a PDF
# whose text contains PRINT_PHRASE.
# Chrome and Flecha: Ctrl+P opens the print preview; Return presses its focused Save button,
# which opens the "Save File" dialog; the path is typed there. Keyboard only: no clicks on
# Chrome's own UI. Pass: that PDF contains PRINT_PHRASE, which it can only do if the preview
# opened and saved.
# CONTROL=1 (run on input-test.html, which lacks the phrase): no Ctrl+P, so neither the
# print line nor a PDF with the phrase may appear.
# Run through xvfb-isolated.sh with ACTIONS=<this file>.
set -euo pipefail
# shellcheck source=lib-actions.sh
. "$(dirname "$0")/lib-actions.sh"
# shellcheck source=auto-lib.sh
. "$(dirname "$0")/auto-lib.sh"
control=${CONTROL:-0}
pdf=$OUT/print-check.pdf
rm -f "$pdf"

app_window >/dev/null
m=$(mark)
((control)) || xdo key ctrl+p

if [[ $APP_MODE == osr ]]; then
    print_started() { (($(count '^\[print\] start' "$m") > 0)); }
    started=no
    wait_until '[print] start' print_started && started=yes
    send "pdf $pdf"
    wait_for '^\[pdf\] ok=' "$m" >/dev/null || true
    if [[ $started == yes ]] && pdf_has_phrase "$pdf"; then
        cell 7 pass "auto, Xvfb: Ctrl+P logged [print] start (the spike has no print dialog or preview); the pdf command wrote a PDF containing the page text"
    else
        cell 7 fail "auto, Xvfb: [print] start after Ctrl+P: $started; PDF from the pdf command contains the page text: $(pdf_has_phrase "$pdf" && echo yes || echo no)"
    fi
    exit 0
fi

# The Save button is disabled until the preview has rendered, so Return is repeated until the
# dialog exists rather than timed.
save_dialog() {
    xdo search --onlyvisible --name '^Save File$' >/dev/null && return 0
    xdo key Return
    return 1
}
if wait_until 'the Save File dialog' save_dialog; then
    dialog=$(xdo search --onlyvisible --name '^Save File$' | sed -n 1p)
    xdo windowactivate --sync "$dialog" 2>/dev/null || xdo windowfocus --sync "$dialog"
    xdo key ctrl+a
    xdo type --delay 20 "$pdf"
    xdo key Return
fi
if wait_until 'a PDF with the page text' pdf_has_phrase "$pdf"; then
    cell 7 pass "auto, Xvfb, keyboard only: Ctrl+P opened the print preview; Return (Save) and a typed path in the Save File dialog wrote a PDF containing the page text"
else
    cell 7 fail "auto, Xvfb, keyboard only: no PDF with the page text after Ctrl+P, Return and the Save File dialog"
fi
