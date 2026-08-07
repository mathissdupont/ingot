# The reference MCP server, in an image `ingot run --sandbox` can contain.
#
#     docker build -f tools/mcp-fs.Dockerfile -t ingot/mcp-fs:0.2 .
#
# Built from source rather than shipped prebuilt: the binary in the image should
# be the one in the repository you are looking at.
#
# The image carries no data and declares no volumes. Everything the server can
# reach is mounted by `ingot run --sandbox`, from the agent's own policy, and
# the container is started read-only with no capabilities and — unless the
# policy grants `network` — no network.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p ingot-mcp --bin ingot-mcp-fs

FROM debian:bookworm-slim
COPY --from=build /src/target/release/ingot-mcp-fs /usr/local/bin/ingot-mcp-fs

# No ENTRYPOINT: the command comes from the manifest, so that an operator can
# see in one place what is started and with what arguments.
