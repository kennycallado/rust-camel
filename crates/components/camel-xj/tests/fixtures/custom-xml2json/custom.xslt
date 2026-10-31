<?xml version="1.0" encoding="UTF-8"?>
<xsl:stylesheet version="3.0"
  xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:template match="/order">
    <data id="{@id}">
      <name><xsl:value-of select="item"/></name>
      <count><xsl:value-of select="qty"/></count>
    </data>
  </xsl:template>
</xsl:stylesheet>
