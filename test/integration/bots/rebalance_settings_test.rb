# frozen_string_literal: true

require 'test_helper'

# The rebalance widget has to look like every other rule on the panel: the explanatory line sits in
# a `small-info` block *below* the sentence (that class is what makes it full-width instead of
# flowing into the sentence), it appears only while the rule is active, and the trigger readout goes
# green when the criteria are currently met — the same contract _indicator_limit_info and
# _price_drop_limit_info follow.
class Bots::RebalanceSettingsTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @bot = create(:dca_dual_asset, user: @user, status: :stopped)
    sign_in @user
  end

  test 'the widget renders on the dual-asset panel' do
    get bot_path(id: @bot.id)

    assert_select 'input[name=?]', 'bots_dca_dual_asset[rebalance_enabled]'
    assert_select 'input[name=?]', 'bots_dca_dual_asset[rebalance_threshold]'
  end

  test 'an inactive rule shows no info line at all' do
    get bot_path(id: @bot.id)

    assert_select '#settings-rebalance-info', count: 0
  end

  test 'an active rule puts its info in a small-info block below the sentence' do
    enable_rebalancing
    stub_drift(0.169)

    get bot_path(id: @bot.id)

    assert_select 'small#settings-rebalance-info.small-info' do
      assert_select 'div', text: /off its target split/
      assert_select 'div', text: /DCA schedule is stopped/
    end
  end

  test 'the drift readout goes green once the criteria are met' do
    enable_rebalancing(threshold: 0.05)
    stub_drift(0.169)

    get bot_path(id: @bot.id)

    assert_select '#settings-rebalance-info div.text-success', text: /off its target split/
  end

  test 'the drift readout stays plain while the rule has not tripped' do
    enable_rebalancing(threshold: 0.05)
    stub_drift(0.02)

    get bot_path(id: @bot.id)

    assert_select '#settings-rebalance-info div.text-success', count: 0
    assert_select '#settings-rebalance-info', text: /off its target split/
  end

  test 'a halted rule offers the resume control instead of a drift reading' do
    enable_rebalancing
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_AMBIGUOUS)

    get bot_path(id: @bot.id)

    assert_select '#settings-rebalance-info div.text-error'
    # A link, not a nested <form>: this partial renders inside the settings form_with, and a browser
    # drops the inner form, which would wire the button to the settings route instead.
    assert_select '#settings-rebalance-info a[href=?]', bot_rebalance_resolutions_path(bot_id: @bot.id)
    assert_select '#settings-rebalance-info form', count: 0
  end

  private

  def enable_rebalancing(threshold: 0.05)
    @bot.settings = @bot.settings.merge('rebalance_enabled' => true, 'rebalance_threshold' => threshold)
    @bot.set_missed_quote_amount
    @bot.save!
  end

  def stub_drift(value)
    Bots::DcaDualAsset.any_instance.stubs(:rebalance_drift).returns(value.to_d)
  end
end
