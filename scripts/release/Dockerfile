# Build environment for the Linux bundles (deb, rpm, AppImage) and the
# Windows NSIS installer (cross-compiled with cargo-xwin). macOS bundles
# cannot be built here; they need a Mac (see docs/RELEASE.md).
#
# Ubuntu 22.04 keeps the glibc floor low, so the Linux bundles run on
# older distributions too.
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH \
    APPIMAGE_EXTRACT_AND_RUN=1

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl wget file git build-essential pkg-config \
      libwebkit2gtk-4.1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev \
      librsvg2-dev libdbus-1-dev rpm xdg-utils \
      clang lld llvm nsis unzip \
    && rm -rf /var/lib/apt/lists/*

# Ubuntu's NSIS 3.08 lacks Win\RestartManager.nsh, which Tauri's installer
# script includes; take the headers and plugins from NSIS 3.10.
RUN curl -fsSL -o /tmp/nsis.zip https://downloads.sourceforge.net/project/nsis/NSIS%203/3.10/nsis-3.10.zip \
    && unzip -q /tmp/nsis.zip -d /tmp \
    && cp -r /tmp/nsis-3.10/Include/. /usr/share/nsis/Include/ \
    && cp -r /tmp/nsis-3.10/Plugins/. /usr/share/nsis/Plugins/ \
    && rm -rf /tmp/nsis.zip /tmp/nsis-3.10

# cargo-xwin calls clang-cl / llvm-lib / lld-link; Ubuntu ships them versioned.
RUN for t in clang-cl llvm-lib llvm-rc lld-link; do \
      [ -e /usr/bin/$t ] || ln -s "$(ls /usr/bin/$t-* | head -1)" /usr/bin/$t; \
    done

RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
    && rustup target add x86_64-pc-windows-msvc \
    && cargo install --locked cargo-xwin

WORKDIR /src
