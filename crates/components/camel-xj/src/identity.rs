// Converts arbitrary XML to JSON using recursive XSLT 3.0 templates.
// fn:xml-to-json() is NOT used here because it requires W3C JSON-XML namespace
// input — this stylesheet handles generic XML structures instead.
pub const XML_TO_JSON_XSLT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xsl:stylesheet version="3.0"
  xmlns:xsl="http://www.w3.org/1999/XSL/Transform"
  xmlns:xj="http://camel.apache.org/component/xj">
  <xsl:output method="text" encoding="UTF-8"/>

  <xsl:template match="/">
    <xsl:choose>
      <xsl:when test="/*/@xj:type">
        <xsl:apply-templates select="/*" mode="xj-reverse"/>
      </xsl:when>
      <xsl:otherwise>
        <xsl:apply-templates select="*"/>
      </xsl:otherwise>
    </xsl:choose>
  </xsl:template>

  <xsl:template name="json-escape-string">
    <xsl:param name="s"/>
    <xsl:value-of select="replace(replace(replace($s, '\\', '\\\\'), '&quot;', '\\&quot;'), '&#10;', '\\n')"/>
  </xsl:template>

  <!-- Root element: wraps in outer object -->
  <xsl:template match="/*">
    <xsl:text>{</xsl:text>
    <xsl:call-template name="element-content"/>
    <xsl:text>}</xsl:text>
  </xsl:template>

  <!-- Named template: emit key + value for current element -->
  <xsl:template name="element-content">
    <xsl:text>"</xsl:text>
    <xsl:value-of select="local-name()"/>
    <xsl:text>":</xsl:text>
    <xsl:choose>
      <!-- Element with child elements: emit nested object -->
      <xsl:when test="*">
        <xsl:text>{</xsl:text>
        <xsl:for-each select="*">
          <xsl:if test="position() > 1"><xsl:text>,</xsl:text></xsl:if>
          <xsl:call-template name="element-content"/>
        </xsl:for-each>
        <xsl:text>}</xsl:text>
      </xsl:when>
      <!-- Leaf element: emit string value -->
      <xsl:otherwise>
        <xsl:text>"</xsl:text>
        <xsl:value-of select="normalize-space(.)"/>
        <xsl:text>"</xsl:text>
      </xsl:otherwise>
    </xsl:choose>
  </xsl:template>

  <xsl:template match="*[@xj:type='object']" mode="xj-reverse">
    <xsl:if test="@xj:name">
      <xsl:text>"</xsl:text><xsl:value-of select="@xj:name"/><xsl:text>":</xsl:text>
    </xsl:if>
    <xsl:text>{</xsl:text>
    <xsl:for-each select="*">
      <xsl:if test="position() > 1"><xsl:text>,</xsl:text></xsl:if>
      <xsl:apply-templates select="." mode="xj-reverse"/>
    </xsl:for-each>
    <xsl:text>}</xsl:text>
  </xsl:template>

  <xsl:template match="*[@xj:type='array']" mode="xj-reverse">
    <xsl:if test="@xj:name">
      <xsl:text>"</xsl:text><xsl:value-of select="@xj:name"/><xsl:text>":</xsl:text>
    </xsl:if>
    <xsl:text>[</xsl:text>
    <xsl:for-each select="*">
      <xsl:if test="position() > 1"><xsl:text>,</xsl:text></xsl:if>
      <xsl:apply-templates select="." mode="xj-reverse"/>
    </xsl:for-each>
    <xsl:text>]</xsl:text>
  </xsl:template>

  <xsl:template match="*[@xj:type='string']" mode="xj-reverse">
    <xsl:if test="@xj:name">
      <xsl:text>"</xsl:text><xsl:value-of select="@xj:name"/><xsl:text>":</xsl:text>
    </xsl:if>
    <xsl:text>"</xsl:text>
    <xsl:call-template name="json-escape-string">
      <xsl:with-param name="s" select="string(.)"/>
    </xsl:call-template>
    <xsl:text>"</xsl:text>
  </xsl:template>

  <xsl:template match="*[@xj:type='int' or @xj:type='float' or @xj:type='number']" mode="xj-reverse">
    <xsl:if test="@xj:name">
      <xsl:text>"</xsl:text><xsl:value-of select="@xj:name"/><xsl:text>":</xsl:text>
    </xsl:if>
    <xsl:value-of select="."/>
  </xsl:template>

  <xsl:template match="*[@xj:type='boolean']" mode="xj-reverse">
    <xsl:if test="@xj:name">
      <xsl:text>"</xsl:text><xsl:value-of select="@xj:name"/><xsl:text>":</xsl:text>
    </xsl:if>
    <xsl:value-of select="."/>
  </xsl:template>

  <xsl:template match="*[@xj:type='null']" mode="xj-reverse">
    <xsl:if test="@xj:name">
      <xsl:text>"</xsl:text><xsl:value-of select="@xj:name"/><xsl:text>":</xsl:text>
    </xsl:if>
    <xsl:text>null</xsl:text>
  </xsl:template>
</xsl:stylesheet>
"#;

pub const JSON_TO_XML_XSLT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xsl:stylesheet version="3.0"
  xmlns:xsl="http://www.w3.org/1999/XSL/Transform"
  xmlns:fn="http://www.w3.org/2005/xpath-functions"
  xmlns:xj="http://camel.apache.org/component/xj"
  exclude-result-prefixes="fn">
  <xsl:output method="xml" encoding="UTF-8" indent="yes"/>
  <xsl:param name="jsonInput"/>

  <xsl:template match="/">
    <xsl:variable name="json-doc" select="json-to-xml($jsonInput)"/>
    <xsl:apply-templates select="$json-doc/*" mode="camel-xj"/>
  </xsl:template>

  <xsl:template match="fn:map" mode="camel-xj">
    <object xj:type="object">
      <xsl:if test="@key">
        <xsl:attribute name="xj:name" namespace="http://camel.apache.org/component/xj" select="@key"/>
      </xsl:if>
      <xsl:apply-templates select="*" mode="camel-xj"/>
    </object>
  </xsl:template>

  <xsl:template match="fn:array" mode="camel-xj">
    <object xj:type="array">
      <xsl:if test="@key">
        <xsl:attribute name="xj:name" namespace="http://camel.apache.org/component/xj" select="@key"/>
      </xsl:if>
      <xsl:apply-templates select="*" mode="camel-xj"/>
    </object>
  </xsl:template>

  <xsl:template match="fn:string" mode="camel-xj">
    <object xj:type="string">
      <xsl:if test="@key">
        <xsl:attribute name="xj:name" namespace="http://camel.apache.org/component/xj" select="@key"/>
      </xsl:if>
      <xsl:value-of select="."/>
    </object>
  </xsl:template>

  <xsl:template match="fn:number" mode="camel-xj">
    <object>
      <xsl:if test="@key">
        <xsl:attribute name="xj:name" namespace="http://camel.apache.org/component/xj" select="@key"/>
      </xsl:if>
      <xsl:attribute name="xj:type" namespace="http://camel.apache.org/component/xj">
        <xsl:choose>
          <xsl:when test="contains(string(.), '.') or contains(string(.), 'e') or contains(string(.), 'E')">float</xsl:when>
          <xsl:otherwise>int</xsl:otherwise>
        </xsl:choose>
      </xsl:attribute>
      <xsl:value-of select="."/>
    </object>
  </xsl:template>

  <xsl:template match="fn:boolean" mode="camel-xj">
    <object xj:type="boolean">
      <xsl:if test="@key">
        <xsl:attribute name="xj:name" namespace="http://camel.apache.org/component/xj" select="@key"/>
      </xsl:if>
      <xsl:value-of select="."/>
    </object>
  </xsl:template>

  <xsl:template match="fn:null" mode="camel-xj">
    <object xj:type="null">
      <xsl:if test="@key">
        <xsl:attribute name="xj:name" namespace="http://camel.apache.org/component/xj" select="@key"/>
      </xsl:if>
      <xsl:text>null</xsl:text>
    </object>
  </xsl:template>
</xsl:stylesheet>
"#;
