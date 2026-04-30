// Converts arbitrary XML to JSON using recursive XSLT 3.0 templates.
// fn:xml-to-json() is NOT used here because it requires W3C JSON-XML namespace
// input — this stylesheet handles generic XML structures instead.
//
// JSON mapping convention (compatibility mode, same as Apache Camel Java xj):
//   - Attributes: "@attrName" keys
//   - Text content: "#text" key (only when element has attrs or children)
//   - Repeated siblings: grouped into JSON arrays
//   - Self-closing with no attrs: null
//   - Simple leaf (no attrs, no children): plain string
pub const XML_TO_JSON_XSLT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
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
    <xsl:variable name="v1" select="replace($s, '\\', '\\\\')"/>
    <xsl:variable name="v2" select="replace($v1, '&quot;', '\\&quot;')"/>
    <xsl:variable name="v3" select="replace($v2, '&#13;', '\\r')"/>
    <xsl:variable name="v4" select="replace($v3, '&#10;', '\\n')"/>
    <xsl:variable name="v5" select="replace($v4, '&#9;', '\\t')"/>
    <xsl:variable name="v6" select="replace($v5, '&#8;', '\\b')"/>
    <xsl:variable name="v7" select="replace($v6, '&#12;', '\\f')"/>
    <xsl:value-of select="$v7"/>
  </xsl:template>

  <xsl:template name="emit-json-string">
    <xsl:param name="s"/>
    <xsl:text>"</xsl:text>
    <xsl:call-template name="json-escape-string">
      <xsl:with-param name="s" select="$s"/>
    </xsl:call-template>
    <xsl:text>"</xsl:text>
  </xsl:template>

  <xsl:template match="/*">
    <xsl:text>{</xsl:text>
    <xsl:call-template name="element-content"/>
    <xsl:text>}</xsl:text>
  </xsl:template>

  <xsl:template name="element-content">
    <xsl:call-template name="emit-json-string">
      <xsl:with-param name="s" select="local-name()"/>
    </xsl:call-template>
    <xsl:text>:</xsl:text>
    <xsl:call-template name="element-value"/>
  </xsl:template>

  <xsl:template name="element-value">
    <xsl:choose>
      <xsl:when test="not(node()) and not(@*)">
        <xsl:text>null</xsl:text>
      </xsl:when>
      <xsl:when test="not(element()) and not(@*)">
        <xsl:call-template name="emit-json-string">
          <xsl:with-param name="s" select="string(.)"/>
        </xsl:call-template>
      </xsl:when>
      <xsl:otherwise>
        <xsl:call-template name="emit-object"/>
      </xsl:otherwise>
    </xsl:choose>
  </xsl:template>

  <xsl:template name="emit-object">
    <xsl:variable name="hasAttrs" select="exists(@*)"/>
    <xsl:variable name="hasText" select="exists(text()[normalize-space()])"/>
    <xsl:variable name="hasChildren" select="exists(element())"/>
    <xsl:text>{</xsl:text>
    <xsl:for-each select="@*">
      <xsl:if test="position() > 1"><xsl:text>,</xsl:text></xsl:if>
      <xsl:call-template name="emit-json-string">
        <xsl:with-param name="s" select="concat('@', local-name())"/>
      </xsl:call-template>
      <xsl:text>:</xsl:text>
      <xsl:call-template name="emit-json-string">
        <xsl:with-param name="s" select="string(.)"/>
      </xsl:call-template>
    </xsl:for-each>
    <xsl:if test="$hasText">
      <xsl:if test="$hasAttrs"><xsl:text>,</xsl:text></xsl:if>
      <xsl:text>"#text":</xsl:text>
      <xsl:call-template name="emit-json-string">
        <xsl:with-param name="s" select="string-join(text(),'')"/>
      </xsl:call-template>
    </xsl:if>
    <xsl:if test="$hasChildren">
      <xsl:if test="$hasAttrs or $hasText"><xsl:text>,</xsl:text></xsl:if>
      <xsl:for-each-group select="*" group-by="local-name()">
        <xsl:if test="position() > 1"><xsl:text>,</xsl:text></xsl:if>
        <xsl:call-template name="emit-json-string">
          <xsl:with-param name="s" select="local-name()"/>
        </xsl:call-template>
        <xsl:text>:</xsl:text>
        <xsl:choose>
          <xsl:when test="count(current-group()) > 1">
            <xsl:text>[</xsl:text>
            <xsl:for-each select="current-group()">
              <xsl:if test="position() > 1"><xsl:text>,</xsl:text></xsl:if>
              <xsl:call-template name="element-value"/>
            </xsl:for-each>
            <xsl:text>]</xsl:text>
          </xsl:when>
          <xsl:otherwise>
            <xsl:for-each select="current-group()[1]">
              <xsl:call-template name="element-value"/>
            </xsl:for-each>
          </xsl:otherwise>
        </xsl:choose>
      </xsl:for-each-group>
    </xsl:if>
    <xsl:text>}</xsl:text>
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
"##;

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
