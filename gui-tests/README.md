## Running gui-tests in docker

The easiest way to run the gui-tests in a stable environment setup as they
expect is by spinning up a temporary web service with the correct data in
docker.

This is supported by the `in-docker` script. It has three phases that can be
run, to allow quicker feedback when editing the gui-tests themselves.

```console
# initialize a temporary database and web service and builds some crates in it
> gui-tests/in-docker init
...
 ✔ Container docsrs-db-1             Healthy
 ✔ Container docsrs-s3-1             Healthy
 ✔ Container docsrs-gui-tests-web-1  Healthy

# while you have changes to make
# do
  # edit your tests
  > vim gui-tests/basic.goml

  # run the tests against the temporary web service
  > gui-tests/in-docker run
  ...
  Running 2 docs.rs GUI (2 concurrently) ...
  ..         (2/2)
# done

# tear down the temporary database and web service
> gui-tests/in-docker cleanup
...
Removed `local/gui-tests` successfully.
```

Running with `all` or without an argument will run all steps in sequence,
skipping the cleanup if it fails so you can inspect the failure. Useful if
you've done some other changes and want to run the gui-tests but aren't
expecting them to fail.

If you are changing the web service or doc builder, take a look in the script at
the steps that `init` takes, you can likely run just one of these steps manually
within your edit-test loop rather than having to fully reinit the setup
(remember to use `--build` to ensure docker-compose rebuilds the image from your
updated source).

```console
# e.g. after editing the doc builder
docker compose run --build --rm gui-tests-builder build crate sysinfo 0.23.4
docker compose run --build --rm gui-tests-builder build crate sysinfo 0.23.5

# or after editing the web service
docker compose up --build --wait --wait-timeout 10 gui-tests-web
```

The web service is also bound onto `localhost:3001` so that you can manually
inspect the pages if necessary.
