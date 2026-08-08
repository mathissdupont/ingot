# The image `ingot run --contained` runs an agent inside.
#
#     docker build -f tools/ingot.Dockerfile -t ingot/run:0.3.0 .
#
# Built from source rather than shipped prebuilt, for the same reason as
# `mcp-fs.Dockerfile`: the `ingot` inside the image and the `ingot` supervising
# it from outside must be the same version, and the supervisor refuses the run
# if the protocol versions disagree. Building from the checkout you are looking
# at is how that stays true.
#
# The image carries two binaries and no data:
#
#   ingot         the interpreter, invoked as `ingot exec` by the supervisor
#   ingot-mcp-fs  the reference filesystem tool server
#
# A manifest naming some other MCP server needs an image with that server in it,
# because a contained run starts its tool servers as children of the contained
# interpreter rather than on the host. Copy this file and add what you need.
#
# Nothing is mounted here and no volume is declared. Everything the agent can
# reach is mounted by `ingot run --contained` from the agent's own `policy`
# block, and the container starts read-only, with no capabilities, and — unless
# the policy grants `network` — with no network at all. The model call does not
# need one: it leaves through the supervisor on the standard streams, which is
# also why no credential is ever placed in here.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
# `--no-default-features` drops the HTTP providers and the TLS stack with them.
# The contained half never calls a provider: it asks the supervisor, which holds
# the credential. Leaving the network providers out means there is no code in the
# image that could use a key even if one somehow arrived.
RUN cargo build --release --no-default-features -p ingot-cli --bin ingot \
 && cargo build --release -p ingot-mcp --bin ingot-mcp-fs

FROM debian:bookworm-slim
COPY --from=build /src/target/release/ingot /usr/local/bin/ingot
COPY --from=build /src/target/release/ingot-mcp-fs /usr/local/bin/ingot-mcp-fs

# The workspace is mounted here, and the supervisor sets `--workdir` to it, so a
# tool server's `--root .` means the workspace on every machine.
WORKDIR /workspace

# No ENTRYPOINT. The command is `ingot exec`, and it is passed explicitly by the
# supervisor so that what runs inside is visible in the invocation rather than
# baked into the image.
