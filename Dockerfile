FROM rust:1.88-slim-bookworm AS build-env

WORKDIR /src

# Install dependencies
RUN apt-get update && \
    apt-get install -y curl build-essential libssl-dev texinfo libcap2-bin pkg-config

# Copy only rust-toolchain.toml for better caching
COPY ./rust-toolchain.toml ./rust-toolchain.toml

# Set environment variables
ENV PATH="/root/.cargo/bin:${PATH}" \
    RUSTUP_HOME="/root/.rustup" \
    CARGO_HOME="/root/.cargo"

# Install the toolchain
RUN rustup component add cargo

# TODO: Hacky but it works
RUN mkdir -p ./src \
    && mkdir -p ./crates/base-api-types/src \
    && mkdir -p ./crates/postgres-docker-utils/src \
    && mkdir -p ./crates/tx-sitter-client/src

# Copy only Cargo.toml for better caching
COPY .cargo/config.toml .cargo/config.toml
COPY ./Cargo.toml ./Cargo.toml
COPY ./Cargo.lock ./Cargo.lock
COPY ./crates/base-api-types/Cargo.toml ./crates/base-api-types/Cargo.toml
COPY ./crates/postgres-docker-utils/Cargo.toml ./crates/postgres-docker-utils/Cargo.toml
COPY ./crates/tx-sitter-client/Cargo.toml ./crates/tx-sitter-client/Cargo.toml

RUN echo "fn main() {}" > ./src/main.rs \
    && echo "fn main() {}" > ./crates/base-api-types/src/main.rs \
    && echo "fn main() {}" > ./crates/postgres-docker-utils/src/main.rs \
    && echo "fn main() {}" > ./crates/tx-sitter-client/src/main.rs

# Prebuild dependencies
RUN cargo fetch \
    && cargo build --release --no-default-features

# Copy all the source files
# .dockerignore ignores the target dir
COPY . .

# Build the binary
RUN cargo fetch \
    && cargo build --release --no-default-features

# Make sure it runs
RUN /src/target/release/tx-sitter --version

# cc variant because we need libgcc and others
FROM gcr.io/distroless/cc-debian12:nonroot

# Copy the tx-sitter binary
COPY --from=build-env --chown=0:10001 --chmod=010 /src/target/release/tx-sitter /bin/tx-sitter

ENTRYPOINT [ "/bin/tx-sitter" ]
