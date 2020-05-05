FROM postgres:alpine
EXPOSE 5432

COPY ./postgres/install_extensions.sql /docker-entrypoint-initdb.d/
