FROM python:3.9-slim AS base

WORKDIR /opt/stress_test

COPY . .

RUN pip install --upgrade pip
RUN pip install poetry
RUN poetry update

# Create new user
RUN groupadd -r appuser && useradd -r -g appuser appuser

# Switch to a non-root user
USER appuser

# Expose the port the app runs on
EXPOSE 7080

# Specify the entrypoint and default command-line arguments
ENTRYPOINT ["python", "stress_test.py"]
CMD ["--help"]
