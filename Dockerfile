FROM debian:12 as build-env

WORKDIR /src

# Install dependencies
RUN apt-get update && \
    apt-get install -y curl build-essential libssl-dev texinfo libcap2-bin pkg-config

# TODO: Use a specific version of rustup
# Install rustup
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y

# Copy only rust-toolchain.toml for better caching
COPY ./rust-toolchain.toml ./rust-toolchain.toml

# Set environment variables
ENV PATH="/root/.cargo/bin:${PATH}"
ENV RUSTUP_HOME="/root/.rustup"
ENV CARGO_HOME="/root/.cargo"

# Install the toolchain
RUN rustup component add cargo

# Copy all the source files
# .dockerignore ignores the target dir
COPY . .

# Build the binary
RUN cargo build --release

# Make sure it runs
RUN /src/target/release/tx-sitter --version

# cc variant because we need libgcc and others
FROM gcr.io/distroless/cc-debian12:nonroot

# Copy the tx-sitter binary
COPY --from=build-env --chown=0:10001 --chmod=010 /src/target/release/tx-sitter /bin/tx-sitter

ENTRYPOINT [ "/bin/tx-sitter" ]
