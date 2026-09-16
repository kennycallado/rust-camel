# In-process routing

These components move Exchanges between routes inside one process. They use in-memory channels and shared state. No network is involved.

- [Direct](direct.md). Synchronous in-process routing.
- [SEDA](seda.md). Asynchronous staging between routes.
- [ControlBus](controlbus.md). Runtime control messages.
- [Master](master.md). Leader-only route execution.