require 'test_helper'

# The connect step: every place that asks for an API key renders the same card — a brand mark,
# "Connect X", the fields, one Connect button that reads "Validating…" while the key is checked,
# and the venue's walkthrough as help under a small label. These pin the shape in its four
# different homes: a bot wizard, the Alpaca variant with its Paper/Live switch, the index wizard's
# CoinGecko step, and the tracker's CoinGecko modal.
class ConnectStepTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    # In tests api keys count as correct (dry run), which would skip the form entirely.
    ApiKey.any_instance.stubs(:correct?).returns(false)
  end

  test 'an exchange key step is the connect card, standard-size fields, help under a label' do
    seed_single_asset_wizard(create(:binance_exchange))

    get new_bots_dca_single_assets_add_api_key_path
    assert_response :success

    assert_select '.set-api.set-api--connect' do
      assert_select '.set-api__brand .set-api__mark svg', count: 1
      assert_select 'h2.set-api__title', text: 'Connect Binance'
      assert_select '.set-api__header', count: 0

      assert_select 'input[name="api_key[key]"].form__input[autofocus]'
      assert_select 'input[name="api_key[secret]"].form__input'
      assert_select '.form__input--large', count: 0

      assert_select 'input[type=submit][value=Connect].button--success[data-turbo-submits-with="Validating…"]'
      assert_select 'input[type=submit].button--large', count: 0

      assert_select '.set-api__instructions' do
        assert_select 'h3.set-api__eyebrow', text: 'How to get API keys from Binance'
        assert_select 'ol.set__list li', minimum: 3
      end
    end
  end

  test 'the Alpaca step asks Paper or Live with the segmented control, posted through a hidden field' do
    seed_alpaca_wizard

    get new_bots_dca_single_assets_add_api_key_path
    assert_response :success

    assert_select '.set-api--connect' do
      assert_select 'h2.set-api__title', text: 'Connect Alpaca'
      assert_select 'select[name="api_key[passphrase]"]', count: 0
      assert_select '[data-controller="form--segmented-field"]' do
        assert_select '.segmented[role=radiogroup] .segmented__option', count: 2
        assert_select '.segmented__option.is-on[data-value=paper]', text: 'Paper'
        assert_select '.segmented__option[data-value=live]', text: 'Live'
        assert_select 'input[type=hidden][name="api_key[passphrase]"][value=paper]'
      end
      # One line of help, not a numbered list.
      assert_select '.set-api__instructions' do
        assert_select 'h3.set-api__eyebrow', text: 'How to get API keys from Alpaca'
        assert_select 'p a[href="https://app.alpaca.markets/"]', text: 'Alpaca'
        assert_select 'ol', count: 0
      end
    end
  end

  test 'the index wizard CoinGecko step is the same card with one field' do
    get new_bots_dca_indexes_setup_coingecko_path
    assert_response :success

    assert_select '.bot-creation-layout.bot-creation-layout--api .set-api--connect' do
      assert_select '.set-api__mark svg', count: 1
      assert_select 'h2.set-api__title', text: 'Connect CoinGecko'
      assert_select 'input[name=api_key].form__input[autofocus][required]', count: 1
      assert_select '.form__input--large', count: 0
      assert_select 'input[type=submit][value=Connect][data-turbo-submits-with="Validating…"]'
      assert_select '.set-api__instructions' do
        assert_select 'h3.set-api__eyebrow', text: 'How to get API keys from CoinGecko'
        assert_select 'ol li', count: 7
      end
    end
    # Explained on the view before this one, so the card says nothing twice.
    assert_no_match I18n.t('setup.api_configuration'), response.body
  end

  test 'a member sees the CoinGecko card without a form, and who to ask' do
    sign_in create(:user, setup_completed: true)

    get new_bots_dca_indexes_setup_coingecko_path
    assert_response :success

    assert_select '.set-api--connect' do
      assert_select 'h2.set-api__title', text: 'Connect CoinGecko'
      assert_select 'form', count: 0
      assert_select '.set-api__note', text: I18n.t('setup.coingecko_admin_only')
    end
  end

  test "the tracker's CoinGecko modal is the same card, keeping its own explanation" do
    get export_modal_tracker_path
    assert_response :success

    assert_select '.modal .set-api--connect' do
      assert_select 'h2.set-api__title', text: 'Connect CoinGecko'
      assert_select '.set-api__note', minimum: 1
      assert_select 'input[name=api_key].form__input[autofocus]', count: 1
      assert_select 'input[type=submit][value=Connect][data-turbo-submits-with="Validating…"]'
      assert_select '.set-api__instructions ol li', count: 7
    end
  end

  private

  def seed_single_asset_wizard(exchange)
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    create(:ticker, :btc_usd, exchange: exchange, base_asset: btc, quote_asset: usd)

    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: btc.id } }
    post advance_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: exchange.id } }
  end

  # A single stock venue: Next on the asset step auto-selects Alpaca into the session.
  def seed_alpaca_wizard
    usd = create(:asset, :usd)
    aapl = create(:asset, symbol: 'AAPL', name: 'Apple Inc', category: 'Stock', external_id: 'aapl')
    alpaca = create(:alpaca_exchange)
    create(:ticker, exchange: alpaca, base_asset: aapl, quote_asset: usd, base: 'AAPL', quote: 'USD')

    post bots_dca_single_assets_order_path, params: { flow: 'asset_first' }
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: aapl.id } }
    post advance_bots_dca_single_assets_pick_buyable_asset_path
  end
end
