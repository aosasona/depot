FROM python:3.13-alpine

WORKDIR /srv
COPY index.html /srv/index.html
USER 65532:65532
EXPOSE 8080
CMD ["python", "-m", "http.server", "8080", "--bind", "0.0.0.0"]
