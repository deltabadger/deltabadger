require 'test_helper'

class Bots::OrdersExportTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    @bitcoin = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    sign_in @user

    exchange = create(:binance_exchange)
    create(:api_key, user: @user, exchange: exchange, key_type: :trading, status: :correct)
    @bot = create(:dca_single_asset,
                  user: @user, exchange: exchange,
                  base_asset: @bitcoin, quote_asset: @usd,
                  status: :stopped, with_api_key: false)
  end

  test 'CSV import opens and submits through Stimulus, not inline handlers' do
    get bot_path(id: @bot.id)

    assert_response :ok
    assert_select 'form[data-controller=?]', 'file-picker'
    assert_select 'button[data-action=?]', 'click->file-picker#open'
    assert_select 'input[type=file][data-file-picker-target=?][data-action=?]',
                  'input', 'change->file-picker#submit'
    assert_select 'button[onclick]', false, 'an inline handler cannot run under the policy'
    assert_select 'input[onchange]', false, 'an inline handler cannot run under the policy'
  end
end
