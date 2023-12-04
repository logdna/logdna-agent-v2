FROM python:3.9-slim AS base

ARG BUILD_TIMESTAMP
ARG BUILD_VERSION

LABEL org.opencontainers.image.created="${BUILD_TIMESTAMP}"
LABEL org.opencontainers.image.version="${BUILD_VERSION}"

WORKDIR /opt/stress_test

COPY . .

RUN pip install --upgrade pip
RUN pip install poetry
RUN poetry install

# Create new user
RUN groupadd -r appuser && useradd -r -g appuser appuser

# Switch to a non-root user
USER appuser

EXPOSE 7080

ENTRYPOINT ["python", "stress_test.py"]
