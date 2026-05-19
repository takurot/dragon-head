FROM ubuntu:24.04

ARG DEBIAN_FRONTEND=noninteractive

# System packages: Chrome, Python 3, build essentials
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
    wget \
    && rm -rf /var/lib/apt/lists/*

# Google Chrome (matches CI runner package)
RUN wget -q -O /tmp/chrome.deb \
    "https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb" \
    && apt-get update \
    && apt-get install -y --no-install-recommends /tmp/chrome.deb \
    && rm /tmp/chrome.deb \
    && rm -rf /var/lib/apt/lists/*

# Rust (stable, matching rust-toolchain.toml)
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain stable --profile minimal \
    && /root/.cargo/bin/rustup component add clippy rustfmt

ENV PATH="/root/.cargo/bin:${PATH}"

# cargo-nextest (pinned to version used in CI)
RUN cargo install cargo-nextest --locked --version 0.9.133

WORKDIR /workspace
