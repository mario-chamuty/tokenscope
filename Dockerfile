FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y \
    build-essential curl wget file pkg-config \
    libssl-dev \
    libgtk-3-dev \
    libwebkit2gtk-4.1-dev \
    libjavascriptcoregtk-4.1-dev \
    libsoup-3.0-dev \
    libayatana-appindicator3-dev \
    librsvg2-dev \
    patchelf \
    libfuse2 \
    && rm -rf /var/lib/apt/lists/*

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# Install tauri-cli
RUN cargo install tauri-cli --version "^2"

WORKDIR /app
COPY . .

# Override bundle targets for Linux
RUN sed -i 's/"targets": \["nsis", "msi"\]/"targets": ["deb", "appimage"]/' src-tauri/tauri.conf.json

RUN cargo tauri build --bundles deb,appimage

CMD ["echo", "Build complete. Output in target/release/bundle/"]
