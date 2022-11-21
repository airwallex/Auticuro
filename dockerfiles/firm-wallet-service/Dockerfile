FROM asia.gcr.io/airwallex/fp-infra-run-image:20.04

ADD ./firm-wallet-service /wallet/firm-wallet-service
ADD ./entrypoint.sh /wallet/entrypoint.sh

WORKDIR /wallet
RUN chmod 777 /wallet

ENTRYPOINT ["/wallet/entrypoint.sh"]
