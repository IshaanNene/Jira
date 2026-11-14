# syntax=docker/dockerfile:1.7
# The single-page app as static files. In production the same build is served from object
# storage behind a CDN; this image is for clusters without one (and kind).
#   docker build -f infrastructure/docker/web.Dockerfile -t web .
FROM node:22-bookworm-slim AS build
ENV CI=true
RUN corepack enable && corepack prepare pnpm@11.23.0 --activate
WORKDIR /src/apps/web
COPY apps/web/package.json apps/web/pnpm-lock.yaml apps/web/pnpm-workspace.yaml ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store pnpm install --frozen-lockfile
COPY apps/web ./
ARG VITE_PRODUCT_NAME=Meridian
ENV VITE_PRODUCT_NAME=${VITE_PRODUCT_NAME}
RUN pnpm build

FROM nginxinc/nginx-unprivileged:1.30-alpine
# Take Alpine security fixes published after the base image was built.
USER root
RUN apk upgrade --no-cache
COPY infrastructure/docker/web.nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /src/apps/web/dist /usr/share/nginx/html
USER 101
EXPOSE 8080
