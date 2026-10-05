# Tauri v2 Linux build environment (Debian 12 / bookworm).
# Builds .deb and .AppImage bundles for TokenScope.
FROM rust:1-bookworm

ENV DEBIAN_FRONTEND=noninteractive

# Tauri v2 Linux system dependencies + bundler tooling.
#   webkit2gtk 4.1 / gtk3            -> webview runtime
#   ayatana-appindicator / librsvg  -> tray icon support
#   patchelf / squashfs / fuse      -> AppImage packaging (linuxdeploy)
#   rsync                           -> fast source sync from the read-only mount
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        curl \
        wget \
        file \
        pkg-config \
        libssl-dev \
        libgtk-3-dev \
        libwebkit2gtk-4.1-dev \
        libayatana-appindicator3-dev \
        librsvg2-dev \
        libxdo-dev \
        patchelf \
        squashfs-tools \
        desktop-file-utils \
        xdg-utils \
        libfuse2 \
        rsync \
    && rm -rf /var/lib/apt/lists/*

# Tauri CLI as a cargo subcommand (`cargo tauri build`), pinned to v2.
RUN cargo install tauri-cli --version "^2.0" --locked

COPY build-entry.sh /usr/local/bin/build-entry.sh
RUN chmod +x /usr/local/bin/build-entry.sh

ENTRYPOINT ["/usr/local/bin/build-entry.sh"]
