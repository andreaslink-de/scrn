# Security policy

## Reporting a vulnerability

Please do not open a public issue for security problems.

Report them privately through GitHub: open the repository's **Security** tab
and choose **Report a vulnerability**. Include what you found, how to
reproduce it and which version or commit you tested.

You can expect a first answer within a week. Once the problem is
confirmed, a fix is prepared and released, and the advisory is published
together with the release. Credit is given unless you prefer otherwise.

## Supported versions

Only the latest release receives security fixes.

## Scope

In scope is everything in this repository: the server, the Docker image
and the deployment documentation. Reports are especially welcome on

- authentication, sessions, passkeys and API tokens,
- the separation of private and public screenshots,
- upload validation and image processing,
- injection in pages, headers or redirects.

Problems in a reverse proxy or host setup that does not follow the README
are out of scope.
