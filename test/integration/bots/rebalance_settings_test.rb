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
    @bot = create(:dca_multi_asset, user: @user, status: :stopped)
    sign_in @user
  end

  test 'the widget renders on the basket panel' do
    get bot_path(id: @bot.id)

    assert_select 'input[name=?]', 'bots_dca_multi_asset[rebalance_enabled]'
    assert_select 'input[name=?]', 'bots_dca_multi_asset[rebalance_threshold]'
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

  test 'the standing note comes before the live readout' do
    enable_rebalancing
    stub_drift(0.169)

    get bot_path(id: @bot.id)

    lines = css_select('#settings-rebalance-info div').map(&:text).map(&:strip)
    assert_match(/DCA schedule is stopped/, lines.first)
    assert_match(/off its target split/, lines.second)
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

  # A halt blocks every tick until the user resolves it, so its Resume lives in the metrics panel, which
  # every direction and every rule state shows — not in this rule, which a selling basket hides and a
  # user can switch off mid-halt.
  test 'a halted rule shows no drift reading, and its Resume is in the metrics panel' do
    enable_rebalancing
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_AMBIGUOUS)

    get bot_path(id: @bot.id)

    assert_select '#settings-rebalance-info .text-error', count: 0
    assert_select '#settings-rebalance-info a[href=?]', bot_rebalance_resolutions_path(bot_id: @bot.id), count: 0
    assert_select '#metrics .text-error'
    # button_to is fine here: the metrics panel is outside the settings form_with.
    assert_select '#metrics form[action=?]', bot_rebalance_resolutions_path(bot_id: @bot.id), count: 1
  end

  test 'a halt stays resolvable after the rule is switched off' do
    enable_rebalancing
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_AMBIGUOUS)
    @bot.set_missed_quote_amount
    @bot.update!(rebalance_enabled: false)

    get bot_path(id: @bot.id)

    assert_select '#metrics form[action=?]', bot_rebalance_resolutions_path(bot_id: @bot.id), count: 1
  end

  test 'a halt stays resolvable with balances hidden' do
    enable_rebalancing
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_AMBIGUOUS)
    @user.update!(hide_balances: true)

    get bot_path(id: @bot.id)

    assert_select '#metrics form[action=?]', bot_rebalance_resolutions_path(bot_id: @bot.id), count: 1
  end

  # == The index bot runs the same widget off the same shared concern ==

  test 'the widget renders on the index panel too' do
    index = create(:dca_index, user: @user, status: :stopped)

    get bot_path(id: index.id)

    assert_select 'input[name=?]', 'bots_dca_index[rebalance_enabled]'
    assert_select 'input[name=?]', 'bots_dca_index[rebalance_threshold]'
  end

  test 'a halted index offers its Resume in the same metrics panel' do
    index = create(:dca_index, user: @user, status: :stopped)
    index.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_AMBIGUOUS)

    get bot_path(id: index.id)

    assert_select '#metrics form[action=?]', bot_rebalance_resolutions_path(bot_id: index.id), count: 1
  end

  test 'an active index rule shows the same small-info block' do
    index = create(:dca_index, user: @user, status: :stopped)
    index.update_columns(settings: index.settings.merge('rebalance_enabled' => true,
                                                        'rebalance_threshold' => 0.05))
    Bots::DcaIndex.any_instance.stubs(:rebalance_drift).returns(0.12.to_d)

    get bot_path(id: index.id)

    assert_select 'small#settings-rebalance-info.small-info' do
      assert_select 'div', text: /DCA schedule is stopped/
      assert_select 'div.text-success', text: /off its target split/
    end
  end

  private

  def enable_rebalancing(threshold: 0.05)
    @bot.settings = @bot.settings.merge('rebalance_enabled' => true, 'rebalance_threshold' => threshold)
    @bot.set_missed_quote_amount
    @bot.save!
  end

  def stub_drift(value)
    Bots::DcaMultiAsset.any_instance.stubs(:rebalance_drift).returns(value.to_d)
  end
end
