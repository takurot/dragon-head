FROM ubuntu:24.04

ARG DEBIAN_FRONTEND=noninteractive

# Fail a `curl ... | sh` pipeline if the curl side fails, instead of `sh`
# silently receiving empty input and the RUN step still exiting 0 with a
# half-installed toolchain. The default Dockerfile shell (/bin/sh -> dash)
# doesn't support `pipefail`; bash does.
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# System packages: Chrome/Chromium, Python 3, build essentials
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    ca-certificates \
    curl \
    git \
    gnupg \
    libssl-dev \
    pkg-config \
    python3 \
    python3-pip \
    sudo \
    wget \
    && rm -rf /var/lib/apt/lists/*

# Google Chrome (amd64 only) via Google's officially signed APT repo — apt
# verifies the repo's Release/Packages metadata against this GPG key before
# trusting any package from it, unlike the previous plain `wget` + `dpkg -i`
# of a loose .deb with no integrity check at all.
#
# ARM64 intentionally gets no browser installed here, not a substitute
# package: Google publishes no official Linux ARM64 Chrome build (not even
# an unsigned snapshot — checked https://storage.googleapis.com/chromium-browser-snapshots/,
# there is no Linux_Arm64 channel), and Ubuntu's own "chromium-browser"
# package is a transitional launcher for the Chromium *snap* on every
# architecture since 22.04 — it stages a working shim at /usr/bin/chromium-browser
# but snapd doesn't run inside this container, so invoking it would fail at
# launch. Installing that stub would be worse than installing nothing: this
# project's own core_runtime::chrome_detection / test_bench_support::should_skip_browser_tests()
# only check that the path *exists*, so the broken stub would defeat their
# graceful-skip behavior and browser-dependent tests would fail outright
# instead of skipping cleanly. Apple Silicon users who need a working
# browser in-container should add `"runArgs": ["--platform=linux/amd64"]`
# to devcontainer.json and run the amd64 image under emulation.
RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
    case "${arch}" in \
      amd64) \
        wget -q -O - https://dl.google.com/linux/linux_signing_key.pub \
          | gpg --dearmor -o /usr/share/keyrings/google-chrome.gpg; \
        echo "deb [arch=amd64 signed-by=/usr/share/keyrings/google-chrome.gpg] http://dl.google.com/linux/chrome/deb/ stable main" \
          > /etc/apt/sources.list.d/google-chrome.list; \
        apt-get update; \
        apt-get install -y --no-install-recommends google-chrome-stable; \
        rm -rf /var/lib/apt/lists/*; \
        ;; \
      arm64) \
        echo "No browser installed on arm64 — see the comment above this RUN step." >&2; \
        ;; \
      *) \
        echo "Unsupported architecture for Chrome: ${arch}" >&2; \
        exit 1; \
        ;; \
    esac

# Non-root user (standard VS Code Dev Containers convention: uid/gid 1000
# matches the default first user on Linux/macOS Docker hosts, so bind-mounted
# workspace files stay writable without a permission fixup step). Everything
# below this point — rustup, cargo-nextest — installs as this user instead
# of root, so the container isn't a root shell by default.
ARG USERNAME=vscode
ARG USER_UID=1000
ARG USER_GID=$USER_UID
# Ubuntu's official 24.04 base image ships a pre-created "ubuntu" user/group
# at uid/gid 1000 (for cloud-init) — remove it first so useradd/groupadd
# below don't collide with it.
RUN if getent passwd ubuntu >/dev/null; then userdel -r ubuntu; fi \
    && if getent group ubuntu >/dev/null; then groupdel ubuntu; fi \
    && groupadd --gid $USER_GID $USERNAME \
    && useradd --uid $USER_UID --gid $USER_GID -m $USERNAME \
    && echo "$USERNAME ALL=(root) NOPASSWD:ALL" > /etc/sudoers.d/$USERNAME \
    && chmod 0440 /etc/sudoers.d/$USERNAME

USER $USERNAME
WORKDIR /home/$USERNAME

# Rust (stable, matching rust-toolchain.toml). rustup's own docs recommend
# this exact HTTPS+TLS1.2-pinned curl|sh invocation as the standard install
# method; there is no independently-published checksum for the rolling
# rustup-init script to pin against (unlike Chrome's GPG-signed repo above).
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain stable --profile minimal \
    && "$HOME/.cargo/bin/rustup" component add clippy rustfmt

ENV PATH="/home/${USERNAME}/.cargo/bin:${PATH}"

# cargo-nextest (pinned to version used in CI)
RUN cargo install cargo-nextest --locked --version 0.9.133

WORKDIR /workspace
