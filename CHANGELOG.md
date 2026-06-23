# Changelog

## [Unreleased]

* Drop version suffix from feature names
* Lock modules behind the respective feature
* Lock `electrum-client` dependency behind `electrs_0_11_1` feature
* Add `BitcoinD::invalidate_blocks`
* Replace `ElectrsD` free-function wait helpers with `ElectrsD::wait_until_*` methods that an optional `timeout` parameter.
* Make `ElectrsD` block waits verify both expected height and block hash using Electrum header notifications.
* Add `ElectrsD` reorg coverage to verify the indexer follows replacement chain tips.
