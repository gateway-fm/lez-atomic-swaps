#include "logos_host.h"

#include <QRandomGenerator>
#include <QTimer>

#include "stub_backends.h"

namespace {
int latencyMs()
{
    return 250 + QRandomGenerator::global()->bounded(350);
}
}

LogosHost::LogosHost(QObject* parent)
    : QObject(parent),
      maker_(new MakerStubBackend(this)),
      taker_(new TakerStubBackend(this))
{
}

LogosHost::~LogosHost() = default;

QObject* LogosHost::module(const QString& name)
{
    if (name == QStringLiteral("lez_atomic_swap_maker"))
        return maker_.data();
    if (name == QStringLiteral("lez_atomic_swap_taker"))
        return taker_.data();
    return nullptr;
}

bool LogosHost::isViewModuleReady(const QString& name) const
{
    if (name == QStringLiteral("lez_atomic_swap_maker"))
        return makerReady_;
    if (name == QStringLiteral("lez_atomic_swap_taker"))
        return takerReady_;
    return false;
}

void LogosHost::watch(const QVariant& operation, const QJSValue& onSuccess,
                      const QJSValue& onError)
{
    if (!operation.canConvert<QString>()) {
        QTimer::singleShot(latencyMs(), [onError]() {
            QJSValue callback = onError;
            callback.call(QJSValueList{QJSValue(
                QStringLiteral("stub host: operation is not watchable"))});
        });
        return;
    }
    const QString value = operation.toString();
    QTimer::singleShot(latencyMs(), [value, onSuccess]() {
        QJSValue callback = onSuccess;
        callback.call(QJSValueList{QJSValue(value)});
    });
}

void LogosHost::markReady(const QString& name)
{
    if (name == QStringLiteral("lez_atomic_swap_maker") && !makerReady_) {
        makerReady_ = true;
        emit viewModuleReadyChanged(name, true);
    } else if (name == QStringLiteral("lez_atomic_swap_taker") && !takerReady_) {
        takerReady_ = true;
        emit viewModuleReadyChanged(name, true);
    }
}
