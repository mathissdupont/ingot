# The proxy a bounded tool server's traffic leaves through.
#
#     docker build -f tools/egress.Dockerfile -t ingot/egress:0.4.0-rc.2 .
#
# Its own image, containing one binary and nothing else. This process sits on
# the network edge of a boundary — it is the only thing in the arrangement that
# can reach both the contained server and the internet — so everything that does
# not need to be here is a reason for it to be somewhere else. No interpreter,
# no model providers, no TLS stack, no tool server, and `ingot-egress` itself
# has no Rust dependencies at all.
#
# Built from source rather than shipped prebuilt, for the same reason as
# `ingot.Dockerfile`: what enforces a policy should be what you can read.
#
# It holds no credential and reaches no filesystem. It is given a host list on
# its command line and it refuses everything else, including — because a policy
# grants names — any request that names an address instead.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p ingot-egress --bin ingot-egress

FROM debian:bookworm-slim
# glibc, for DNS. The proxy resolving names is the whole point: a client that
# resolved its own would let the check and the connection disagree.
COPY --from=build /src/target/release/ingot-egress /usr/local/bin/ingot-egress

# Nothing here needs a name, a home directory or a privilege.
USER 65534:65534
ENTRYPOINT ["/usr/local/bin/ingot-egress"]
