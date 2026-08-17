FROM node:22-alpine

LABEL org.logos-co.atomic-swaps.purpose="local-lez-explorer"

WORKDIR /app
COPY server.js .
COPY public public/

ENV LEZ_INDEXER_URL=http://indexer:8779
ENV LEZ_EXPLORER_PORT=3003
EXPOSE 3003
USER node
CMD ["node", "server.js"]
