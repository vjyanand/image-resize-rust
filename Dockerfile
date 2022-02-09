FROM alpine:latest

RUN apk add --update --no-cache --repository http://dl-3.alpinelinux.org/alpine/edge/community --repository http://dl-3.alpinelinux.org/alpine/edge/main vips-dev gcc musl-dev rust cargo openssl-dev

WORKDIR /opt/image-size

COPY ./Cargo.toml ./Cargo.toml

ADD . ./

RUN cargo build --release

EXPOSE 8080

CMD ["./target/release/image"]


