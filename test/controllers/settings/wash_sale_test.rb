require 'test_helper'

class Settings::WashSaleTest < ActionDispatch::IntegrationTest
  include ActiveJob::TestHelper

  # The app pins the SolidQueue adapter, and ActiveJob::TestHelper leaves a configured adapter alone.
  def queue_adapter_for_test
    ActiveJob::QueueAdapters::TestAdapter.new
  end

  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    sign_in @user
  end

  test 'the account box asks exactly what the modal asks' do
    get settings_account_path

    assert_response :success
    # One question, one partial, one wording.
    assert_select '#wash_sale .wash-sale-question'
    assert_select '#wash_sale form select[name=?]', 'wash_sale[jurisdiction]' do
      assert_select 'option', count: Tax::Jurisdictions.wash_sale_options.size
    end
    assert_select "#wash_sale input[name='wash_sale[enabled]']", count: 2
  end

  test 'undecided preselects neither answer and leaves Confirm dead' do
    get settings_account_path

    assert_select "#wash_sale input[name='wash_sale[enabled]'][checked]", count: 0,
                                                                          message: 'a preselected answer is not a decision'
    assert_select '#wash_sale button[type=submit][disabled]'
  end

  test 'a decided account sees its own answer, and can change it' do
    @user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'IE')

    get settings_account_path

    assert_select "#wash_sale input[name='wash_sale[enabled]'][value='1'][checked]"
    assert_select '#wash_sale button[type=submit][disabled]', count: 0
    assert_select '#wash_sale option[value=IE][selected]'
  end

  test 'the jurisdiction pill can show a country other than the one it was rendered with' do
    get settings_account_path

    # The select is invisible; the label beside it is what the user reads. Nothing here submits on
    # change, so without this wiring the pill keeps the country it was rendered with forever.
    assert_select '#wash_sale .sinput--select[data-controller=?]', 'form--select-display' do
      assert_select 'span[data-form--select-display-target=?]', 'label'
      assert_select 'select[data-action=?]', 'change->form--select-display#update'
    end
  end

  test 'the button matches the rest of the page' do
    get settings_account_path

    # Full width comes from .setting-form, like every other widget on this page.
    assert_select '#wash_sale form.setting-form button.button--sky'
  end

  test 'an empty answer changes nothing' do
    patch settings_update_wash_sale_path, params: { wash_sale: { jurisdiction: 'IE' } }

    assert_response :unprocessable_entity
    assert_not_predicate @user.reload, :wash_sale_decided?
  end

  test 'Confirm records the no, and the answer sticks' do
    patch settings_update_wash_sale_path, params: { wash_sale: { enabled: '0' } }

    @user.reload
    assert_predicate @user, :wash_sale_decided?
    assert_not_predicate @user, :wash_sale_enabled?

    get settings_account_path
    assert_select "#wash_sale input[name='wash_sale[enabled]'][value='0'][checked]"
  end

  test 'switching it on stores the window and decides' do
    patch settings_update_wash_sale_path,
          params: { wash_sale: { enabled: '1', jurisdiction: 'IE' } }

    assert_equal 28, @user.reload.wash_sale_days
    assert_predicate @user, :wash_sale_decided?
  end

  test 'switching it off keeps the chosen window for next time' do
    @user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'IE')

    patch settings_update_wash_sale_path, params: { wash_sale: { enabled: '0' } }

    assert_equal 0, @user.reload.wash_sale_days
    assert_equal 'IE', @user.wash_sale_jurisdiction, 'the choice survives the switch'
  end

  test 'turning it on re-arms from history' do
    assert_enqueued_with(job: Tracker::LedgerJob) do
      patch settings_update_wash_sale_path, params: { wash_sale: { enabled: '1' } }
    end
  end

  test 'turning it off enqueues no walk' do
    @user.update!(wash_sale_enabled: true)

    assert_no_enqueued_jobs(only: Tracker::LedgerJob) do
      patch settings_update_wash_sale_path, params: { wash_sale: { enabled: '0' } }
    end
  end

  test 'an unknown code is refused without changing anything' do
    patch settings_update_wash_sale_path,
          params: { wash_sale: { enabled: '1', jurisdiction: 'XX' } }

    assert_response :unprocessable_entity
    assert_not_predicate @user.reload, :wash_sale_enabled?
  end

  test 'the box lists what is locked right now, across every bot' do
    @user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    asset = create(:asset, symbol: 'AAA', name: 'Coin AAA', external_id: 'coin-aaa')
    WashSaleLock.create!(user: @user, asset: asset, buy_locked_until: (Date.current + 12).beginning_of_day)

    get settings_account_path

    assert_select '#wash_sale .settings-locked li', text: /AAA/
    assert_select '#wash_sale .settings-locked li', text: /12/
  end

  test 'with nothing locked the list says so rather than showing an empty box' do
    @user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')

    get settings_account_path

    assert_select '#wash_sale .settings-locked li', count: 0
    assert_select '#wash_sale', text: /#{I18n.t('settings.wash_sale.locked_none')}/
  end
  test 'the window reads as days and a country code' do
    get settings_account_path

    assert_select '#wash_sale option', text: '30 days (US)'
    assert_select '#wash_sale option', text: '30 days (UK)'
    assert_select '#wash_sale option', text: '28 days (IE)'
  end
end
