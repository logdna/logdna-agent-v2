FROM python:3.8-slim AS base

WORKDIR /opt/stress_test

RUN pip install --upgrade pip
RUN pip install Flask
RUN pip install ratelimiter

COPY stress_test.py .

# Switch to a non-root user
USER 1001

# Expose the port the app runs on
EXPOSE 7080

# Specify the entrypoint and default command-line arguments
ENTRYPOINT ["python", "stress_test.py"]
CMD ["--help"]
