#pragma once

#include <QJSValue>
#include <QObject>
#include <QScopedPointer>
#include <QString>
#include <QVariant>

class MakerStubBackend;
class TakerStubBackend;

class LogosHost : public QObject
{
    Q_OBJECT
public:
    explicit LogosHost(QObject* parent = nullptr);
    ~LogosHost() override;

    Q_INVOKABLE QObject* module(const QString& name);
    Q_INVOKABLE bool isViewModuleReady(const QString& name) const;
    Q_INVOKABLE void watch(const QVariant& operation, const QJSValue& onSuccess,
                           const QJSValue& onError);

    void markReady(const QString& name);

signals:
    void viewModuleReadyChanged(const QString& moduleName, bool isReady);

private:
    QScopedPointer<MakerStubBackend> maker_;
    QScopedPointer<TakerStubBackend> taker_;
    bool makerReady_ = false;
    bool takerReady_ = false;
};
