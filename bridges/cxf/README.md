# CXF Bridge

A Quarkus-based gRPC bridge for Apache CXF SOAP services with WS-Security support.

## WS-Security Configuration

The CXF bridge supports WS-Security (signing, encryption, verification, and decryption) via WSS4J. Security is enabled automatically when a keystore path is configured.

### Properties

| Property                                  | Default           | Description                                                                                                                                             |
| ----------------------------------------- | ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cxf.keystore.path`                       | _(none)_          | Path to the JKS keystore file. When set, WS-Security processing is enabled.                                                                             |
| `cxf.keystore.password`                   | _(none)_          | Password to the keystore.                                                                                                                               |
| `cxf.truststore.path`                     | _(none)_          | Path to the truststore for signature verification. Falls back to keystore if not set.                                                                   |
| `cxf.truststore.password`                 | _(none)_          | Password to the truststore.                                                                                                                             |
| `cxf.sig.username`                        | `clientkey`       | Alias of the key entry in the keystore used for signing.                                                                                                |
| `cxf.sig.password`                        | _(none)_          | Password for the private key entry.                                                                                                                     |
| `cxf.enc.username`                        | `serverkey`       | Alias used for encryption (recipient's public key).                                                                                                     |
| `cxf.security.actions.out`                | _(empty)_         | Space-separated WSS4J action tokens for outbound messages (e.g. `Signature`, `Signature Encrypt`).                                                      |
| `cxf.security.actions.in`                 | _(empty)_         | Space-separated WSS4J action tokens for inbound messages.                                                                                               |
| `cxf.security.signature.algorithm`        | _(WSS4J default)_ | Signature algorithm URI (e.g. `http://www.w3.org/2000/09/xmldsig#rsa-sha1` for legacy, `http://www.w3.org/2001/04/xmldsig-more#rsa-sha256` for modern). |
| `cxf.security.signature.digest.algorithm` | _(WSS4J default)_ | Digest algorithm URI (e.g. `http://www.w3.org/2000/09/xmldsig#sha1` or `http://www.w3.org/2001/04/xmlenc#sha256`).                                      |
| `cxf.security.signature.c14n.algorithm`   | _(WSS4J default)_ | Canonicalization algorithm URI.                                                                                                                         |
| `cxf.security.signature.parts`            | _(WSS4J default)_ | SOAP parts to sign in `{Element}{namespace}LocalName` format. Example: `{Element}{http://www.w3.org/2003/05/soap-envelope}Body`.                        |
| `cxf.security.username`                   | _(none)_          | Username for basic authentication.                                                                                                                      |
| `cxf.security.password`                   | _(none)_          | Password for basic authentication.                                                                                                                      |

### Example — minimal signing configuration

```properties
cxf.keystore.path=/etc/camel/keystore.jks
cxf.keystore.password=changeit
cxf.sig.username=myalias
cxf.sig.password=changeit
```

### Example — sign + encrypt outbound, verify + decrypt inbound

```properties
cxf.keystore.path=/etc/camel/keystore.jks
cxf.keystore.password=changeit
cxf.truststore.path=/etc/camel/truststore.jks
cxf.truststore.password=changeit
cxf.sig.username=myalias
cxf.sig.password=changeit
cxf.security.actions.out=Signature Encrypt
cxf.security.actions.in=Signature Encrypt
```

### Example — legacy BUS interop (CAT112 / Baleares 112, rsa-sha1 + sha1)

```properties
cxf.keystore.path=/etc/camel/keystore.jks
cxf.keystore.password=changeit
cxf.sig.username=myalias
cxf.sig.password=changeit
cxf.security.actions.out=Signature
cxf.security.actions.in=Signature
cxf.security.signature.algorithm=http://www.w3.org/2000/09/xmldsig#rsa-sha1
cxf.security.signature.digest.algorithm=http://www.w3.org/2000/09/xmldsig#sha1
cxf.security.signature.parts={Element}{http://www.w3.org/2003/05/soap-envelope}Body
```

### Startup logging

When the bridge starts, it logs one of:

```
WS-Security: ENABLED (signing/verification active)
```

or

```
WS-Security: DISABLED (no keystore configured)
```
