package org.rustcamel.jms;

import io.grpc.Status;
import io.grpc.stub.ServerCallStreamObserver;
import io.grpc.stub.StreamObserver;
import io.quarkus.grpc.GrpcService;
import jakarta.annotation.PreDestroy;
import jakarta.inject.Inject;
import javax.jms.Connection;
import java.util.concurrent.ConcurrentHashMap;
import jms_bridge.BridgeServiceGrpc;
import jms_bridge.HealthRequest;
import jms_bridge.HealthResponse;
import jms_bridge.JmsMessage;
import jms_bridge.SendRequest;
import jms_bridge.SendResponse;
import jms_bridge.SubscribeRequest;

@GrpcService
public class JmsBridgeService extends BridgeServiceGrpc.BridgeServiceImplBase {
    @Inject JmsProducer producer;
    @Inject jakarta.enterprise.inject.Instance<JmsConsumer> consumerFactory;
    @Inject JmsClientFactory clientFactory;

    private final ConcurrentHashMap<String, JmsConsumer> activeConsumers = new ConcurrentHashMap<>();
    private volatile boolean lastHealthy = false;
    private volatile long lastHealthCheck = 0L;
    private volatile String lastHealthMessage = "ok";
    private static final long HEALTH_TTL_MS = 5000L;

    @Override
    public void send(SendRequest request, StreamObserver<SendResponse> responseObserver) {
        try {
            String msgId = producer.send(
                request.getDestination(),
                request.getBody().toByteArray(),
                request.getHeadersMap(),
                request.getContentType()
            );
            responseObserver.onNext(SendResponse.newBuilder().setMessageId(msgId == null ? "" : msgId).build());
            responseObserver.onCompleted();
            lastHealthy = true;
            lastHealthMessage = "ok";
        } catch (Exception e) {
            responseObserver.onError(Status.INTERNAL.withDescription(e.getMessage()).asException());
        }
    }

    @Override
    public void subscribe(SubscribeRequest request, StreamObserver<JmsMessage> responseObserver) {
        JmsConsumer consumer = consumerFactory.get();
        String subId = request.getSubscriptionId();
        activeConsumers.put(subId, consumer);

        if (responseObserver instanceof ServerCallStreamObserver<JmsMessage> serverObs) {
            serverObs.setOnCancelHandler(() -> {
                consumer.stop();
                activeConsumers.remove(subId);
                consumerFactory.destroy(consumer);
            });
        }

        consumer.subscribe(request.getDestination(), subId, new StreamObserver<>() {
            @Override
            public void onNext(JmsMessage msg) {
                responseObserver.onNext(msg);
            }

            @Override
            public void onError(Throwable t) {
                consumer.stop();
                activeConsumers.remove(subId);
                consumerFactory.destroy(consumer);
                responseObserver.onError(t);
            }

            @Override
            public void onCompleted() {
                consumer.stop();
                activeConsumers.remove(subId);
                consumerFactory.destroy(consumer);
                responseObserver.onCompleted();
            }
        });
    }

    @Override
    public void health(HealthRequest request, StreamObserver<HealthResponse> responseObserver) {
        long now = System.currentTimeMillis();
        if (now - lastHealthCheck > HEALTH_TTL_MS) {
            synchronized (this) {
                now = System.currentTimeMillis();
                if (now - lastHealthCheck > HEALTH_TTL_MS) {
                    try (Connection c = clientFactory.get().createConnection()) {
                        c.start();
                        lastHealthy = true;
                        lastHealthMessage = "ok";
                    } catch (Exception e) {
                        lastHealthy = false;
                        lastHealthMessage = e.getMessage() == null ? "connection failed" : e.getMessage();
                    }
                    lastHealthCheck = now;
                }
            }
        }

        responseObserver.onNext(HealthResponse.newBuilder()
            .setHealthy(lastHealthy)
            .setBrokerConnected(lastHealthy)
            .setMessage(lastHealthMessage)
            .build());
        responseObserver.onCompleted();
    }

    @PreDestroy
    public void shutdown() {
        for (JmsConsumer c : activeConsumers.values()) {
            c.stop();
            consumerFactory.destroy(c);
        }
        activeConsumers.clear();
    }
}
