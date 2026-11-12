#!/bin/sh
set -e

if command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -d /usr/share/icons/hicolor ]; then
    gtk-update-icon-cache -f -t /usr/share/icons/hicolor >/dev/null 2>&1 || true
fi

if [ -d /usr/share/icons/hicolor ]; then
    touch -c /usr/share/icons/hicolor >/dev/null 2>&1 || true
fi

if command -v update-desktop-database >/dev/null 2>&1 && [ -d /usr/share/applications ]; then
    update-desktop-database /usr/share/applications >/dev/null 2>&1 || true
fi
