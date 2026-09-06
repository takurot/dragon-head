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

# Google Chrome (amd64) via Google's officially signed APT repo — apt
# verifies the repo's Release/Packages metadata against this GPG key before
# trusting any package from it, unlike the previous plain `wget` + `dpkg -i`
# of a loose .deb with no integrity check at all.
#
# Google does not publish an official Linux ARM64 Chrome build, so ARM64
# hosts (e.g. Apple Silicon running this devcontainer) get Chromium from
# Ubuntu's own signed repo instead — test_bench_support::should_skip_browser_tests()
# already accepts either binary, matching what CI's Linux runners do.
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
        ;; \
      arm64) \
        apt-get update; \
        apt-get install -y --no-install-recommends chromium-browser; \
        ;; \
      *) \
        echo "Unsupported architecture for Chrome/Chromium: ${arch}" >&2; \
        exit 1; \
        ;; \
    esac; \
    rm -rf /var/lib/apt/lists/*

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
