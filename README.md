Fastly Compute@Edge demo
========================

**This is a partial reimplementation of [GOV.UK's WWW VCL](https://github.com/alphagov/govuk-cdn-config/blob/master/vcl_templates/www.vcl.erb) in Rust.  It is not complete.**

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

Then launch the service:

```bash
$ cd rust
$ fastly compute serve
```

You can test mirror fallback by launching the Python server instead at
ports `8889`, `8890`, or `8891`.

You can interact with the server using cURL.

Examples
--------

### Authorization and SSL

In the default configuration, both HTTP Basic Auth and SSL headers are
required:

```bash
$ curl -v -H "Authorization: Basic foo" -H "Fastly-SSL: 1" "http://127.0.0.1:7676"
```

Without the `Authorization` header:

```bash
$ curl -v -H "Fastly-SSL: 1" "http://127.0.0.1:7676"
< HTTP/1.1 401 Unauthorized
< www-authenticate: Basic
< content-length: 0
< date: Tue, 03 Aug 2021 12:30:24 GMT
```

Basic Auth can be disabled by setting `basic_authorization` to `null`
in `rust/config.yaml` and restarting the service.

Without the `Fastly-SSL` header:

```bash
$ curl -v -H "Authorization: Basic foo" "http://127.0.0.1:7676"
< HTTP/1.1 301 Moved Permanently
< location: https://127.0.0.1:7676/
< fastly-backend-name: force_ssl
< content-length: 0
< date: Tue, 03 Aug 2021 12:31:59 GMT
```

This cannot be disabled through the configuration file, but you can
comment out the relevant lines of `rust/src/cdn_rules.rs` if need be.

### Personalisation

This repo was originally an attempt to see if we could use
Compute@Edge for personalisation.  There is a simple version of that
implemented with string replacement.

Without a cookie:

```bash
$ curl -H "Authorization: Basic foo" -H "Fastly-SSL: 1" "http://127.0.0.1:7676/"
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
```

And with a cookie:

```bash
$ curl -H "Authorization: Basic foo" -H "Fastly-SSL: 1" -H "Cookie: govuk_account_session=foo" "http://127.0.0.1:7676/"
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

### Synthetic "not found" responses

You can special-case a path to always return a synthetic 404.  These
are defined in `special_paths.not_found` in `rust/config.yaml`.

```bash
$ curl -v -H "Authorization: Basic foo" -H "Fastly-SSL: 1" "http://127.0.0.1:7676/autodiscover/autodiscover.xml"
< HTTP/1.0 404 Not Found
< server: SimpleHTTP/0.6 Python/3.8.9
< date: Tue, 03 Aug 2021 12:36:08 GMT
< connection: close
< content-type: text/html;charset=utf-8
< content-length: 469
<
<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.01//EN"
        "http://www.w3.org/TR/html4/strict.dtd">
<html>
    <head>
        <meta http-equiv="Content-Type" content="text/html;charset=utf-8">
        <title>Error response</title>
    </head>
    <body>
        <h1>Error response</h1>
        <p>Error code: 404</p>
        <p>Message: File not found.</p>
        <p>Error code explanation: HTTPStatus.NOT_FOUND - Nothing matches the given URI.</p>
    </body>
</html>
```

### Synthetic redirect responses

Similarly, you can special-case redirects, which return a synthetic
302, with `special_paths.redirect`.

```bash
$ curl -v -H "Authorization: Basic foo" -H "Fastly-SSL: 1" "http://127.0.0.1:7676/security.txt"
< HTTP/1.1 302 Found
< location: https://vdp.cabinetoffice.gov.uk/.well-known/security.txt
< content-length: 0
< date: Tue, 03 Aug 2021 12:37:08 GMT
```

### A/B tests

A/B tests are implemented if you have a `cookies_policy` cookie
containing `%22usage%22:true`.

If so, you will be assigned to a random variant in every test, as seen
in the `Set-Cookie` response header:

```bash
 curl -v -H "Authorization: Basic foo" -H "Fastly-SSL: 1" -H "Cookie: cookies_policy=%22usage%22:true" "http://127.0.0.1:7676/"
< HTTP/1.0 200 OK
< server: SimpleHTTP/0.6 Python/3.8.9
< date: Tue, 03 Aug 2021 12:43:57 GMT
< content-type: application/octet-stream
< set-cookie: ABTest-Example=B; secure; max-age=86400
< last-modified: Tue, 03 Aug 2021 12:39:23 GMT
< content-length: 0
```

You can also pass a chosen variant in a query parameter:

```bash
$ curl -v -H "Authorization: Basic foo" -H "Fastly-SSL: 1" -H "Cookie: cookies_policy=%22usage%22:true" "http://127.0.0.1:7676/?ABTest-Example=A"
< HTTP/1.0 200 OK
< server: SimpleHTTP/0.6 Python/3.8.9
< date: Tue, 03 Aug 2021 12:44:53 GMT
< content-type: application/octet-stream
< set-cookie: ABTest-Example=A; secure; max-age=86400
< last-modified: Tue, 03 Aug 2021 12:39:23 GMT
< content-length: 0
```

Or in a cookie:

```bash
$ curl -v -H "Authorization: Basic foo" -H "Fastly-SSL: 1" -H "Cookie: cookies_policy=%22usage%22:true; ABTest-Example=A" "http://127.0.0.1:7676/"
< HTTP/1.0 200 OK
< server: SimpleHTTP/0.6 Python/3.8.9
< date: Tue, 03 Aug 2021 12:58:19 GMT
< content-type: application/octet-stream
< set-cookie: ABTest-Example=A; secure; max-age=86400; path=/
< last-modified: Tue, 03 Aug 2021 12:39:23 GMT
< content-length: 0
```

For the `/help/ab-testing` path, the `cookies_policy` is not needed.

### Falling back to the mirrors

The service will fall back in this order:

1. Try `localhost:8888`, and on server error:
2. Try `localhost:8889`, and on server error:
3. Try `localhost:8890`, and on server error:
4. Try `localhost:8891`, and on server error:
5. Return a synthetic 503 response

The backend used is given in the `Fastly-Backend-Name` header.  You
can try this out by stopping the services:

```bash
$ curl -v -H "Authorization: Basic foo" -H "Fastly-SSL: 1" "http://127.0.0.1:7676/"
< HTTP/1.0 200 OK
< server: SimpleHTTP/0.6 Python/3.8.9
< date: Tue, 03 Aug 2021 13:05:49 GMT
< content-type: text/html
< fastly-backend-name: mirrorGCS
< last-modified: Mon, 02 Aug 2021 15:25:15 GMT
< fastly-failover: 1
< content-length: 9
```


Testing
-------

There are no tests.

This *could* be tested by launching the service and having a program
which makes requests and checks the responses are what's expected.
