require 'test_helper'

class ExchangeProxyTest < ActiveSupport::TestCase
  def with_env(key, value)
    old_value = ENV[key]
    value.nil? ? ENV.delete(key) : ENV[key] = value
    yield
  ensure
    old_value.nil? ? ENV.delete(key) : ENV[key] = old_value
  end

  test 'AppConfig proxy wins over ENV' do
    AppConfig.set('proxy_binance', 'http://claimed-proxy.test:8101')

    with_env('PROXY_BINANCE', 'http://fleet-proxy.test:8100') do
      assert_equal 'http://claimed-proxy.test:8101', ExchangeProxy.for('binance')
    end
  end

  test 'blank AppConfig proxy falls through to ENV' do
    AppConfig.set('proxy_binance', '')

    with_env('PROXY_BINANCE', 'http://fleet-proxy.test:8100') do
      assert_equal 'http://fleet-proxy.test:8100', ExchangeProxy.for(:binance)
    end
  end

  test 'blank AppConfig and blank ENV return nil' do
    AppConfig.set('proxy_binance', '')

    with_env('PROXY_BINANCE', '') do
      assert_nil ExchangeProxy.for('binance')
    end
  end

  test 'no AppConfig proxy and no ENV return nil' do
    with_env('PROXY_BINANCE', nil) do
      assert_nil ExchangeProxy.for('binance')
    end
  end

  test 'a fresh exchange uses a newly claimed proxy without a process restart' do
    env_client = mock('env_client')
    claimed_client = mock('claimed_client')

    with_env('PROXY_BINANCE', 'http://fleet-proxy.test:8100') do
      Honeymaker.expects(:client)
                .with('binance', api_key: nil, api_secret: nil, proxy: 'http://fleet-proxy.test:8100')
                .returns(env_client)
      assert_same env_client, build(:binance_exchange).send(:client)

      AppConfig.set('proxy_binance', 'http://claimed-proxy.test:8101')

      Honeymaker.expects(:client)
                .with('binance', api_key: nil, api_secret: nil, proxy: 'http://claimed-proxy.test:8101')
                .returns(claimed_client)
      assert_same claimed_client, build(:binance_exchange).send(:client)
    end
  end
end
