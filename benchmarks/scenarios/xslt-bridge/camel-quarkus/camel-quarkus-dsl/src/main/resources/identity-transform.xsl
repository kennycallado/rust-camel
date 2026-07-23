<?xml version="1.0" encoding="UTF-8"?>
<!--
  T4a XSLT benchmark stylesheet — camel-quarkus-dsl/-native copy.

  Classpath-packaged copy of
  benchmarks/scenarios/xslt-bridge/shared/identity-transform.xsl,
  placed under src/main/resources/ so camel-quarkus-xslt can AOT-compile
  it to a Java translet at build time per the extension's native-mode
  contract (only classpath: URIs are supported in native; file:, http:
  are JVM-only — see https://camel.apache.org/camel-quarkus/latest/reference/extensions/xslt.html).

  Two intentional deltas vs the shared original:
    - version="1.0" (vs 3.0): the Quarkus native pipeline compiles via
      Xalan (XSLT 1.0 only — camel-quarkus-support-xalan installs itself
      as the JAXP TransformerFactory). The identity transform uses zero
      XSLT 3.0 features, so the workload is byte-identical.
    - lives under src/main/resources/ so the Quarkus build sees a
      classpath: URI at AOT-compile time.

  If the shared stylesheet changes, re-sync this copy. The build does
  not auto-sync (kept simple — the file is 1KB and rarely edited).
-->
<xsl:stylesheet version="1.0"
                xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:output method="xml" omit-xml-declaration="yes" indent="no"/>

  <!-- Identity template: copy current node + recurse. -->
  <xsl:template match="@* | node()">
    <xsl:copy>
      <xsl:apply-templates select="@* | node()"/>
    </xsl:copy>
  </xsl:template>
</xsl:stylesheet>
