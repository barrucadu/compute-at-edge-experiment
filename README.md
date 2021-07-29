Fastly Compute@Edge demo
========================

First-time set up
-----------------

[Install the Fastly CLI](https://developer.fastly.com/learning/compute/)

You don't need a Fastly account.

If the CLI isn't available for your platform:

- compile [the CLI](https://github.com/fastly/cli/) and add it to your `$PATH`
- compile [viceroy](https://github.com/fastly/Viceroy) and move the binary to `~/.config/fastly/viceroy`


Running it locally
------------------

Firstly serve the static assets on `localhost:8888`.  For example, with Python 3:

```bash
$ cd static
$ python3 -m http.server 8888
```

Then launch one of the services.

- rust:

    ```bash
    $ cd rust
    $ fastly compute serve
    ```

- or assemblyscript:

    ```bash
    $ cd assemblyscript
    $ fastly compute serve
    ```

Then you can visit:

- `http://127.0.0.1:7676` to see the Compute@Edge response
- `http://127.0.0.1:8888` to see the non-Compute@Edge response (simulating falling back to the static mirrors)

Or use cURL:

```bash
$ curl http://127.0.0.1:8888
<!DOCTYPE html>
<html>
  <head>
    <title>Test Page</title>
    <link rel="stylesheet" href="/style.css">
  </head>

  <body>
    <h1 class="compute_at_edge--show-if-mirrored">This shows on the static mirrors</h1>
    <h1 class="compute_at_edge--show-if-cookie">This shows if you're logged in</h1>
    <h1 class="compute_at_edge--show-if-not-cookie">This shows if you're not logged in</h1>
  </body>
</html>

$ curl http://127.0.0.1:7676
<!DOCTYPE html>
<html>
  <head>
    <title>Test Page</title>
    <link rel="stylesheet" href="/style.css">
  </head>

  <body>
    <h1 class="compute_at_edge--hide">This shows on the static mirrors</h1>
    <h1 class="compute_at_edge--hide">This shows if you're logged in</h1>
    <h1 class="compute_at_edge--show">This shows if you're not logged in</h1>
  </body>
</html>

$ curl -H 'Cookie: govuk_account_session=foo' http://127.0.0.1:7676
<!DOCTYPE html>
<html>
  <head>
    <title>Test Page</title>
    <link rel="stylesheet" href="/style.css">
  </head>

  <body>
    <h1 class="compute_at_edge--hide">This shows on the static mirrors</h1>
    <h1 class="compute_at_edge--show">This shows if you're logged in</h1>
    <h1 class="compute_at_edge--hide">This shows if you're not logged in</h1>
  </body>
</html>
```


Testing
-------

There are no tests.

This *could* be tested by launching the service and having a program
which makes requests and checks the responses are what's expected.
