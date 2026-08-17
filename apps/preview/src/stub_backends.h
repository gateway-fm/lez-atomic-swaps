#pragma once

#include <QObject>
#include <QString>

class MakerStubBackend : public QObject
{
    Q_OBJECT
public:
    explicit MakerStubBackend(QObject* parent = nullptr);

    Q_INVOKABLE QString health();
    Q_INVOKABLE QString saveRoute(QString requestId, QString pair, QString direction,
                                  QString minimumForeignUnits, QString maximumForeignUnits,
                                  QString offerTtlSeconds, QString lezUnitsPerLot,
                                  QString foreignUnitsPerLot);
    Q_INVOKABLE QString history();
    Q_INVOKABLE QString monitor(QString swapId);
    Q_INVOKABLE QString claim(QString requestId, QString swapId, QString expectedGeneration);
    Q_INVOKABLE QString refund(QString requestId, QString swapId, QString expectedGeneration);
};

class TakerStubBackend : public QObject
{
    Q_OBJECT
public:
    explicit TakerStubBackend(QObject* parent = nullptr);

    Q_INVOKABLE QString health();
    Q_INVOKABLE QString listOffers(QString pair, QString direction);
    Q_INVOKABLE QString initiate(QString requestId, QString offerId, QString pair,
                                 QString direction, QString makerIdentity,
                                 QString signedEnvelopeSha256, QString foreignUnits,
                                 QString expectedLezUnits);
    Q_INVOKABLE QString listSwaps();
    Q_INVOKABLE QString monitor(QString swapId);
    Q_INVOKABLE QString claim(QString requestId, QString swapId, QString expectedGeneration);
    Q_INVOKABLE QString refund(QString requestId, QString swapId, QString expectedGeneration);
};
