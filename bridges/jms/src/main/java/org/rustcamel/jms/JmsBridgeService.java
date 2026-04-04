package org.rustcamel.jms;

import io.grpc.Status;
import io.grpc.stub.ServerCallStreamObserver;
import io.grpc.stub.StreamObserver;
import io.quarkus.grpc.GrpcService;
import jakarta.annotation.PreDestroy;
import jakarta.inject.Inject;
import java.util.concurrent.ConcurrentHashMap;
import jms_bridge.BridgeServiceGrpc;
import jms_bridge.HealthRequest;
import jms_bridge.HealthResponse;
import jms_bridge.JmsMessage;
import jms_bridge.SendRequest;
import jms_bridge.SendResponse;
import jms_bridge.SubscribeRequest;
import org.jboss.logging.Logger;

@GrpcService
public class JmsBridgeService extends BridgeServiceGrpc.BridgeServiceImplBase {
    private static final Logger LOG = Logger.getLogger(JmsBridgeService.class);

    @Inject JmsProducer producer;
    @Inject jakarta.enterprise.inject.Instance<JmsConsumer> consumerFactory;
    @Inject JmsClientFactory clientFactory;

    private final ConcurrentHashMap<String, JmsConsumer> activeConsumers = new ConcurrentHashMap<>();
    private volatile boolean lastHealthy = false;
    private volatile long lastHealthCheck = 0L;
    private volatile String lastHealthMessage = "ok";
    private static final long HEALTH_TTL_MS = 10_000L;

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
            LOG.error("JMS send failed for destination '" + request.getDestination() + "': " + e.getMessage(), e);
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
                    try {
                        clientFactory.checkHealth();
                        lastHealthy = true;
                        lastHealthMessage = "ok";
                    } catch (Exception e) {
                        lastHealthy = false;
                        lastHealthMessage = summarizeThrowable(e);
                        LOG.warn("JMS broker health check failed: " + lastHealthMessage);
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

    private static String summarizeThrowable(Throwable error) {
        if (error == null) {
            return "connection failed";
        }

        StringBuilder summary = new StringBuilder();
        Throwable cur = error;
        int depth = 0;
        while (cur != null && depth < 6) {
            if (depth > 0) {
                summary.append(" <- ");
            }
            summary.append(cur.getClass().getName());
            String msg = cur.getMessage();
            if (msg != null && !msg.isBlank()) {
                summary.append(": ").append(msg);
            }
            cur = cur.getCause();
            depth++;
        }

        if (summary.length() == 0) {
            return "connection failed";
        }
        return summary.toString();
    }
}
