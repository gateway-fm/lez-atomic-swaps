FROM python:3.12-alpine

LABEL org.logos-co.atomic-swaps.purpose="owner-local-btc-wallet-market"

WORKDIR /app
COPY --chmod=0555 controller.py /app/controller.py
COPY --chmod=0555 healthcheck.py /app/healthcheck.py

USER 4713:4713
CMD ["python3", "/app/controller.py"]
