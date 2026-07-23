<?xml version="1.0" encoding="UTF-8"?>
<!--
  T4a XSLT benchmark shared stylesheet.

  Identity transform with a single named template that copies the
  input document verbatim. Used by all 4 T4a artifacts (Java
  standalone, Java Quarkus native, rust-camel-lib, rust-camel-cli)
  so the workload is byte-identical across the bridge tax comparison.

  Spec §4.8: pinning Saxon-HE 12.5 across all 4 artifacts. This
  stylesheet is XSLT 3.0-compatible (runs on Saxon-HE 12.5 in both
  the rust-camel xml-bridge and the Apache Camel in-process engine).

  Size: ~1KB. The transform itself is intentionally non-trivial
  (one xsl:template + xsl:copy + xsl:apply-templates, NOT just
  <xsl:copy-of select="/"/>) so the engine does real work per
  invocation — the bridge tax measurement would be meaningless if
  the engine could short-circuit.
-->
<xsl:stylesheet version="3.0"
                xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:output method="xml" omit-xml-declaration="yes" indent="no"/>

  <!-- Identity template: copy current node + recurse. -->
  <xsl:template match="@* | node()">
    <xsl:copy>
      <xsl:apply-templates select="@* | node()"/>
    </xsl:copy>
  </xsl:template>
</xsl:stylesheet>
