FROM asia.gcr.io/airwallex/fp-infra-run-image:20.04

ADD ./firm-wallet-gateway /gateway/firm-wallet-gateway
ADD ./entrypoint.sh /gateway/entrypoint.sh
ADD ./.env ./gateway/.env

WORKDIR /gateway
RUN chmod 777 /gateway

ENV DOWNSTREAM_SERVICE="http://firm-wallet-service-0.firm-wallet-service:20161,\
http://firm-wallet-service-1.firm-wallet-service:20161,http://firm-wallet-service-2.firm-wallet-service:20161,\
http://firm-wallet-service-3.firm-wallet-service:20161,http://firm-wallet-service-4.firm-wallet-service:20161"

ENTRYPOINT ["/gateway/entrypoint.sh"]