#!/bin/sh
# The RPM's automatic requirements (cargo-generate-rpm --auto-req): its builtin procedure,
# except that a weak version reference becomes an ordinary one. File paths come on stdin,
# requirements go to stdout.
#
# cargo-generate-rpm 0.21.0 copies ldd's " [WEAK]" into the name, so a binary built with Rust
# 1.99 (whose std refers to newer glibc symbols weakly) requires libc.so.6(GLIBC_2.18)[WEAK](64bit),
# which nothing provides, and dnf and zypper refuse the package. Remove this script once
# cargo-generate-rpm strips the marker itself (#168).
set -eu

while IFS= read -r file || [ -n "$file" ]; do
    [ -f "$file" ] && [ -x "$file" ] || continue
    if [ "$(head -c 4 "$file" | od -An -c | tr -d ' \n')" != '177ELF' ]; then
        # A script: its interpreter, if it exists.
        if [ "$(head -c 2 "$file")" = '#!' ]; then
            interpreter=$(head -n 1 "$file" | cut -c 3- | awk '{ print $1 }')
            [ -n "$interpreter" ] && [ -e "$interpreter" ] && echo "$interpreter"
        fi
        continue
    fi
    # A 64-bit ELF file's requirements end in (64bit).
    marker=
    [ "$(head -c 5 "$file" | tail -c 1 | od -An -tu1 | tr -d ' ')" = 2 ] && marker='(64bit)'
    LC_ALL=C ldd -v "$file" | awk -v path="$file" -v marker="$marker" '
        function wanted(name) {
            return name ~ /\.so/ && name ~ /^(ld[.-]|ld64[.-]|lib)/
        }
        # The libraries, up to the first empty line.
        part == 0 && /^[[:space:]]*$/ { part = 1; next }
        part == 0 {
            sub(/^[[:space:]]+/, "")
            split($0, f, " ")
            if (wanted(f[1])) print f[1] "()" marker
            next
        }
        # Then the versions this file needs: "libc.so.6 (GLIBC_2.18) [WEAK] => /lib/...".
        part == 1 && index($0, "Version information:") { part = 2; next }
        part == 2 && index($0, path) { part = 3; next }
        part == 3 && index($0, " => ") == 0 { part = 4; next }
        part == 3 {
            sub(/^[[:space:]]+/, "")
            sub(/ => .*/, "")
            sub(/ \[WEAK\]$/, "")
            split($0, f, " ")
            if (!wanted(f[1])) next
            print f[1] "()" marker
            gsub(/ /, "")
            print $0 marker
        }
    '
    # Only a GNU hash table: the dynamic loader must support it.
    sections=$(LC_ALL=C readelf -S -W "$file")
    if echo "$sections" | grep -q ' \.gnu\.hash ' && ! echo "$sections" | grep -q ' \.hash '; then
        echo 'rtld(GNU_HASH)'
    fi
done | sort -u
