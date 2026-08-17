#include <QGuiApplication>
#include <QQmlContext>
#include <QQmlEngine>
#include <QQmlApplicationEngine>
#include <QQuickWindow>
#include <QTimer>
#include <QUrl>

#include "logos_host.h"

int main(int argc, char* argv[])
{
    qputenv("QT_QUICK_CONTROLS_STYLE", "Basic");

    QGuiApplication app(argc, argv);
    app.setApplicationName("lez-atomic-swap-preview");
    app.setOrganizationName("LEZ atomic swaps preview");

    LogosHost logosHost;

    QQmlApplicationEngine engine;
    engine.rootContext()->setContextProperty(QStringLiteral("logos"), &logosHost);
    engine.rootContext()->setContextProperty(
        QStringLiteral("sourceRoot"), QStringLiteral(LEZ_PREVIEW_SOURCE_ROOT));
    engine.rootContext()->setContextProperty(
        QStringLiteral("previewInitialView"),
        QString::fromUtf8(qgetenv("LEZ_PREVIEW_VIEW").isEmpty() ? "maker" : qgetenv("LEZ_PREVIEW_VIEW")));

    QObject::connect(&engine, &QQmlApplicationEngine::objectCreationFailed, &app,
        []() { QCoreApplication::exit(1); }, Qt::QueuedConnection);
    engine.loadFromModule("LezSwapPreview", "Preview");

    QTimer::singleShot(600, &logosHost, [&logosHost]() {
        logosHost.markReady(QStringLiteral("lez_atomic_swap_maker"));
    });
    QTimer::singleShot(900, &logosHost, [&logosHost]() {
        logosHost.markReady(QStringLiteral("lez_atomic_swap_taker"));
    });

    const QByteArray screenshotPath = qgetenv("LEZ_PREVIEW_SCREENSHOT");
    if (!screenshotPath.isEmpty()) {
        QTimer::singleShot(2500, &engine, [&engine, &app, screenshotPath = screenshotPath]() {
            const auto roots = engine.rootObjects();
            if (roots.isEmpty()) {
                qWarning("preview: no root object to grab");
                QCoreApplication::exit(1);
                return;
            }
            auto* window = qobject_cast<QQuickWindow*>(roots.first());
            if (window == nullptr || !window->grabWindow().save(QString::fromUtf8(screenshotPath))) {
                qWarning("preview: failed to save screenshot");
                QCoreApplication::exit(1);
                return;
            }
            QCoreApplication::exit(0);
        });
    }

    return app.exec();
}
