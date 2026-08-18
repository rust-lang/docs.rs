# Nginx

- acts as a reverse proxy to our web server,
- compresses content, and
- authenticates with the CDN.

Before we had the NgWAF, nginx also handled rate limiting and IP blocking during
attacks.

Changes are made manually on the server in `/etc/nginx/`, after which nginx must
be restarted via systemd.

**There is no test system.**
