FROM mcr.microsoft.com/vscode/devcontainers/rust:0-bullseye
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get -y install --no-install-recommends pre-commit
RUN cargo install cargo-edit cargo-upgrades
