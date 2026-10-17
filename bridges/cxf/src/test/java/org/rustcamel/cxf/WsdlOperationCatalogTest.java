package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import jakarta.xml.ws.Service;
import java.net.URL;
import java.util.ArrayList;
import java.util.List;
import java.util.Set;
import javax.xml.namespace.QName;
import javax.xml.transform.Source;
import org.apache.cxf.BusFactory;
import org.apache.cxf.binding.soap.model.SoapOperationInfo;
import org.apache.cxf.jaxws.DispatchImpl;
import org.apache.cxf.service.model.BindingInfo;
import org.apache.cxf.service.model.BindingMessageInfo;
import org.apache.cxf.service.model.BindingOperationInfo;
import org.apache.cxf.service.model.MessageInfo;
import org.apache.cxf.service.model.MessagePartInfo;
import org.apache.cxf.wsdl11.WSDLServiceFactory;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

@DisplayName("WsdlOperationCatalog")
class WsdlOperationCatalogTest {

  private static final String NS = "urn:oneway:363";
  private static final String WSDL = "/soap-oneway.wsdl";

  private static QName q(String local) {
    return new QName(NS, local);
  }

  private static BindingInfo fixtureBinding(String service, String port) {
    URL wsdl = WsdlOperationCatalogTest.class.getResource(WSDL);
    assertNotNull(wsdl, "fixture WSDL must be on the test classpath");
    WSDLServiceFactory factory =
        new WSDLServiceFactory(
            BusFactory.getDefaultBus(), wsdl.toExternalForm(), new QName(NS, service));
    return factory.create().getServiceInfos().get(0).getEndpoint(new QName(NS, port)).getBinding();
  }

  private static BindingInfo mainBinding() {
    return fixtureBinding("OneWayService", "MainPort");
  }

  private static BindingInfo aliasBinding() {
    return fixtureBinding("AliasService", "AliasPort");
  }

  private static BindingInfo mockedBinding(List<BindingOperationInfo> operations) {
    BindingInfo binding = mock(BindingInfo.class);
    when(binding.getOperations()).thenReturn(operations);
    return binding;
  }

  private static BindingOperationInfo mockedOperation(
      String localName, QName inputElement, String action, boolean oneWay) {
    MessagePartInfo part = mock(MessagePartInfo.class);
    when(part.getElementQName()).thenReturn(inputElement);
    MessageInfo messageInfo = mock(MessageInfo.class);
    when(messageInfo.getMessageParts()).thenReturn(new ArrayList<>(List.of(part)));
    BindingMessageInfo input = mock(BindingMessageInfo.class);
    when(input.getMessageInfo()).thenReturn(messageInfo);
    BindingOperationInfo operation = mock(BindingOperationInfo.class);
    when(operation.getName()).thenReturn(new QName(NS, localName));
    when(operation.getInput()).thenReturn(input);
    when(operation.getOutput()).thenReturn(oneWay ? null : mock(BindingMessageInfo.class));
    SoapOperationInfo soap = new SoapOperationInfo();
    soap.setAction(action);
    when(operation.getExtensor(SoapOperationInfo.class)).thenReturn(soap);
    return operation;
  }

  @Test
  @DisplayName("explicit input-only operation uses selected binding")
  void explicitInputOnlyOperationUsesSelectedBinding() {
    WsdlOperationCatalog catalog = WsdlOperationCatalog.fromBinding(mainBinding());

    WsdlOperationCatalog.Operation receive =
        catalog.resolveProducer("ReceiveMessageOp", q("receiveMessage"));
    assertEquals(new QName(NS, "ReceiveMessageOp"), receive.name());
    assertEquals("urn:receive", receive.soapAction());
    assertTrue(receive.oneWay(), "ReceiveMessageOp has input and no output");
    assertEquals(Set.of(q("receiveMessage")), receive.inputQNames());

    WsdlOperationCatalog.Operation echo = catalog.resolveProducer("EchoOp", q("echoRequest"));
    assertEquals(new QName(NS, "EchoOp"), echo.name());
    assertEquals("urn:echo", echo.soapAction());
    assertFalse(echo.oneWay(), "EchoOp declares an output");
  }

  @Test
  @DisplayName("unnamed input uses the qualified wrapped element")
  void unnamedInputUsesQualifiedWrappedElement() {
    WsdlOperationCatalog catalog = WsdlOperationCatalog.fromBinding(mainBinding());

    WsdlOperationCatalog.Operation receive =
        catalog.resolveProducer(null, new QName(NS, "receiveMessage"));
    assertEquals(new QName(NS, "ReceiveMessageOp"), receive.name());

    assertThrows(
        WsdlOperationCatalog.InvalidOperationException.class,
        () -> catalog.resolveProducer(null, new QName("urn:other", "receiveMessage")),
        "an identical local name in another namespace must not match");
  }

  @Test
  @DisplayName("unknown and ambiguous selections fail")
  void unknownAndAmbiguousSelectionsFail() {
    WsdlOperationCatalog main = WsdlOperationCatalog.fromBinding(mainBinding());
    WsdlOperationCatalog alias = WsdlOperationCatalog.fromBinding(aliasBinding());

    assertThrows(
        WsdlOperationCatalog.InvalidOperationException.class,
        () -> main.resolveProducer("MissingOp", null),
        "explicit name absent from the selected binding");
    assertThrows(
        WsdlOperationCatalog.InvalidOperationException.class,
        () -> main.resolveProducer(null, null),
        "no explicit name and no input QName");
    assertThrows(
        WsdlOperationCatalog.InvalidOperationException.class,
        () -> main.resolveProducer(null, new QName(NS, "notThere")),
        "input QName matching no operation");
    assertThrows(
        WsdlOperationCatalog.InvalidOperationException.class,
        () -> alias.resolveProducer(null, q("receiveMessage")),
        "alias binding shares its input QName across operations");

    assertThrows(
        WsdlOperationCatalog.InvalidOperationException.class,
        () -> main.resolveProducer("AliasReceiveMessageOp", null),
        "the alias operation must not be reachable from the main binding");
  }

  @Test
  @DisplayName("consumer action must agree with the unique input")
  void consumerActionMustAgreeWithUniqueInput() {
    WsdlOperationCatalog main = WsdlOperationCatalog.fromBinding(mainBinding());

    assertEquals(
        new QName(NS, "ReceiveMessageOp"),
        main.resolveConsumer(q("receiveMessage"), null).name(),
        "absent action is accepted");
    assertEquals(
        new QName(NS, "ReceiveMessageOp"),
        main.resolveConsumer(q("receiveMessage"), "urn:receive").name(),
        "matching action is accepted");
    assertEquals(
        new QName(NS, "ReceiveMessageOp"),
        main.resolveConsumer(q("receiveMessage"), "\"urn:receive\"").name(),
        "one outer quote pair is normalized away");

    assertThrows(
        WsdlOperationCatalog.InvalidOperationException.class,
        () -> main.resolveConsumer(q("receiveMessage"), "urn:echo"),
        "conflicting action must be rejected");
    assertThrows(
        WsdlOperationCatalog.InvalidOperationException.class,
        () -> main.resolveConsumer(q("receiveMessage"), ""),
        "present empty action disagrees with a declared action");

    WsdlOperationCatalog emptyAction =
        WsdlOperationCatalog.fromBinding(
            mockedBinding(
                new ArrayList<>(
                    List.of(mockedOperation("ReceiveMessageOp", q("receiveMessage"), "", true)))));
    assertEquals(
        new QName(NS, "ReceiveMessageOp"),
        emptyAction.resolveConsumer(q("receiveMessage"), "\"\"").name(),
        "quoted empty agrees with a declared empty action");
  }

  @Test
  @DisplayName("catalog snapshot cannot be mutated")
  void catalogSnapshotCannotBeMutated() {
    QName receiveElement = q("receiveMessage");
    MessagePartInfo part = mock(MessagePartInfo.class);
    when(part.getElementQName()).thenReturn(receiveElement);
    List<MessagePartInfo> parts = new ArrayList<>(List.of(part));
    MessageInfo messageInfo = mock(MessageInfo.class);
    when(messageInfo.getMessageParts()).thenReturn(parts);
    BindingMessageInfo input = mock(BindingMessageInfo.class);
    when(input.getMessageInfo()).thenReturn(messageInfo);
    BindingOperationInfo operation = mock(BindingOperationInfo.class);
    when(operation.getName()).thenReturn(new QName(NS, "ReceiveMessageOp"));
    when(operation.getInput()).thenReturn(input);
    when(operation.getOutput()).thenReturn(null);
    SoapOperationInfo soap = new SoapOperationInfo();
    soap.setAction("urn:receive");
    when(operation.getExtensor(SoapOperationInfo.class)).thenReturn(soap);
    List<BindingOperationInfo> operations = new ArrayList<>(List.of(operation));
    BindingInfo binding = mock(BindingInfo.class);
    when(binding.getOperations()).thenReturn(operations);

    WsdlOperationCatalog catalog = WsdlOperationCatalog.fromBinding(binding);

    operations.clear();
    parts.clear();

    WsdlOperationCatalog.Operation resolved =
        catalog.resolveProducer("ReceiveMessageOp", receiveElement);
    assertEquals(new QName(NS, "ReceiveMessageOp"), resolved.name());
    assertTrue(resolved.oneWay());
    assertEquals(Set.of(receiveElement), resolved.inputQNames());
    assertThrows(
        UnsupportedOperationException.class,
        () -> resolved.inputQNames().add(q("extra")),
        "the returned input QName set must reject mutation");
  }

  private static DispatchImpl<Source> dispatchFor(URL wsdl, String service, String port) {
    QName serviceQName = service.startsWith("{") ? QName.valueOf(service) : new QName(service);
    QName portQName = port.startsWith("{") ? QName.valueOf(port) : new QName(port);
    Service jaxwsService = Service.create(wsdl, serviceQName);
    @SuppressWarnings("unchecked")
    DispatchImpl<Source> dispatch =
        (DispatchImpl<Source>)
            jaxwsService.createDispatch(portQName, Source.class, Service.Mode.PAYLOAD);
    return dispatch;
  }

  @Test
  @DisplayName("load uses the selected port without invoking a temporary dispatch")
  void catalogLoadUsesSelectedPortWithoutInvoking() throws Exception {
    URL wsdl = WsdlOperationCatalogTest.class.getResource(WSDL);
    assertNotNull(wsdl, "fixture WSDL must be on the test classpath");
    String wsdlArg = wsdl.toExternalForm();
    List<DispatchImpl<Source>> temporaryDispatches = new ArrayList<>();
    WsdlOperationCatalog.TemporaryDispatchFactory recordingFactory =
        (wsdlString, service, port) -> {
          DispatchImpl<Source> recording = spy(dispatchFor(wsdl, service, port));
          temporaryDispatches.add(recording);
          return recording;
        };

    WsdlOperationCatalog main =
        WsdlOperationCatalog.load(
            wsdlArg, "{urn:oneway:363}OneWayService", "{urn:oneway:363}MainPort", recordingFactory);
    WsdlOperationCatalog alias =
        WsdlOperationCatalog.load(
            wsdlArg, "{urn:oneway:363}AliasService", "{urn:oneway:363}AliasPort", recordingFactory);

    // Port-isolated metadata: the selected port's binding decides the operation set.
    assertDoesNotThrow(() -> main.resolveProducer("EchoOp", null));
    assertDoesNotThrow(() -> alias.resolveProducer("AliasReceiveMessageOp", null));
    assertThrows(
        WsdlOperationCatalog.InvalidOperationException.class,
        () -> main.resolveProducer("AliasReceiveMessageOp", null),
        "the alias-only operation must not be reachable from the main port");
    assertThrows(
        WsdlOperationCatalog.InvalidOperationException.class,
        () -> alias.resolveProducer("EchoOp", null),
        "the main-only operation must not be reachable from the alias port");

    assertEquals(2, temporaryDispatches.size(), "one temporary dispatch per loaded port");
    for (DispatchImpl<Source> temporary : temporaryDispatches) {
      verify(temporary, never()).invoke(any());
      verify(temporary, never()).invokeOneWay(any());
      verify(temporary, times(1)).close();
    }

    // A failure after the temporary dispatch is created must still close it.
    DispatchImpl<Source> failing =
        spy(dispatchFor(wsdl, "{urn:oneway:363}OneWayService", "{urn:oneway:363}MainPort"));
    doThrow(new IllegalStateException("binding lookup failed")).when(failing).getClient();

    assertThrows(
        IllegalStateException.class,
        () ->
            WsdlOperationCatalog.load(
                wsdlArg,
                "{urn:oneway:363}OneWayService",
                "{urn:oneway:363}MainPort",
                (wsdlString, service, port) -> failing),
        "a metadata load failure must escape the loader");
    verify(failing, never()).invoke(any());
    verify(failing, never()).invokeOneWay(any());
    verify(failing, times(1)).close();
  }
}
