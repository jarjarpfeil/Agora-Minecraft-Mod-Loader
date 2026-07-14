# Third-Party Notices

This file lists third-party software bundled or downloaded by Agora, along
with their license information.

---

## Eclipse Temurin (Adoptium)

Agora downloads and manages private Eclipse Temurin Java Runtime Environment
(JRE) builds from the Adoptium Working Group for use with Minecraft instances.

- **Project:** Eclipse Temurin
- **Source:** https://adoptium.net/temurin/releases/
- **Provider:** Adoptium Working Group (Eclipse Foundation)
- **License:** GNU General Public License, version 2, with the Classpath Exception
  (GPL-2.0-only WITH Classpath-exception-2.0)
- **SPDX Identifier:** `GPL-2.0-only WITH Classpath-exception-2.0`

### License text

The full GPL-2.0 license text is available at:
https://github.com/adoptium/temurin-build/blob/master/LICENSE

The Classpath Exception text is available at:
https://github.com/adoptium/temurin-build/blob/master/LICENSE-CLASSPATH-EXCEPTION

### Runtime archives

Each Temurin JRE archive downloaded by Agora contains the `NOTICE` and
`LICENSE` files distributed by the Eclipse Temurin project. These files are
retained inside the extracted runtime directory under Agora's application data
and are not modified or removed.

### Verification

Adoptium releases pass Java Compatibility Kit (JCK) testing and Adoptium's
additional AQAvit verification. Each runtime entry in Agora's pinned runtime
catalog includes the expected SHA-256 hash and archive size for independent
verification.

### Usage

Agora uses Temurin JRE builds (HotSpot) for the `java` runtime executable only.
No development tools (JDK) are required for launching Minecraft.

Agora does not modify the user's system `PATH`, `JAVA_HOME`, or registry.
Managed runtimes are stored privately in Agora's application data directory.
