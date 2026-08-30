FROM python:3.12-alpine

LABEL org.logos-co.atomic-swaps.purpose="local-demo-allowlisted-docker-launcher"

WORKDIR /app
COPY --chmod=0555 launcher.py /app/launcher.py

# Docker's host socket is root-owned in this explicitly local-only profile.
# hadolint ignore=DL3002
USER 0:0
CMD ["python3", "/app/launcher.py"]
