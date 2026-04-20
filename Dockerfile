# Use Ubuntu 20.04 as the base image
FROM ubuntu:20.04

# Set the environment to noninteractive to avoid prompts
ENV DEBIAN_FRONTEND=noninteractive

# Update and install dependencies in one RUN command
RUN apt-get update && apt-get install -y \
    build-essential \
    clang \
    curl \
    wget \
    cron \
    libpq-dev \
    libssl-dev \
    pkg-config \
    libavutil-dev \
    libavformat-dev \
    libavcodec-dev \
    libavdevice-dev \
    ffmpeg \
    capnproto \
    nodejs \
    python3 \
    python3-pip \
    unzip \
    && rm -rf /var/lib/apt/lists/*

# Install Bun (JS runtime and package manager)
RUN curl -fsSL https://bun.sh/install | bash
ENV PATH="/root/.bun/bin:${PATH}"

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | bash -s -- --default-toolchain 1.88.0 -y

# Ensure that the Rust environment is available in the current session
ENV PATH="/root/.cargo/bin:${PATH}"
ENV CC=clang
ENV CXX=clang++
# Set up a Cargo cache directory
ENV CARGO_HOME=/usr/local/cargo
RUN mkdir -p $CARGO_HOME
VOLUME $CARGO_HOME

# Set the working directory
WORKDIR /usr/src/connect

# Copy only the files needed for dependency installation first
COPY fetch/requirements.txt fetch/requirements.txt
COPY install_deps.sh .
COPY Cargo.toml Cargo.lock ./

# Install Python packages
RUN pip3 install -U "huggingface_hub[cli,hf_transfer]"
RUN pip3 install --no-cache-dir -r fetch/requirements.txt

# Copy the frontend directory separately to support install_deps.sh
COPY frontend/ frontend/

# Set SHELL environment variable (still bash, but pnpm no longer relevant)
ENV SHELL=/bin/bash

# Install JavaScript/TypeScript dependencies in frontend with Bun via install_deps.sh
RUN /bin/bash -c "./install_deps.sh"

# Install Rust tools
RUN /bin/bash -c "source $HOME/.cargo/env && export RUST_MIN_STACK=134217728 && cargo install loco-cli cargo-insta sea-orm-cli"

# Copy migration directory for dependency resolution
COPY migration/ migration/

# Copy just the manifests first
COPY Cargo.toml Cargo.lock ./

# Create an empty src/main.rs if your package needs one.
RUN mkdir -p src && echo "fn main() {}" > src/main.rs

# Pre-fetch all dependencies (this creates a cache layer)
RUN cargo fetch
RUN cargo update

# Now copy the entire source code
COPY . .

# Build the application with necessary features
RUN /bin/bash -c "source $HOME/.cargo/env && export RUST_MIN_STACK=134217728 && cargo build --release"

# Setup cronjob for deleting old files
RUN echo "0 * * * * cd /usr/src/connect && ./target/release/connect-cli task deleter >> /var/log/cron.log 2>&1" > /etc/cron.d/connect-cron
RUN chmod 0644 /etc/cron.d/connect-cron
RUN crontab /etc/cron.d/connect-cron

# Expose the ports your server runs on
# HTTPS
EXPOSE 3222
EXPOSE 3223
# HTTP
EXPOSE 3111
EXPOSE 3112
