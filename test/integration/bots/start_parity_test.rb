# frozen_string_literal: true

require 'test_helper'

# MCP and REST start are the web Start's twins: the same start_fresh choice, read the same way and
# handed to the same Bot#start, behind the same refusals. One bot per path, identical before the
# start, must come out identical after it — every column start writes, the carry it captured, the
# tick it armed and the activity row it logged.
class Bots::StartParityTest < ActionDispatch::IntegrationTest
  TURBO_STREAM_ACCEPT = 'text/vnd.turbo-stream.html, text/html'
  SUNDAY = Time.utc(2026, 5, 24, 12, 34, 0)

  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @user.set_mcp_tool_enabled('start_bot', true)
    @user.set_rest_tool_enabled('start_bot', true)
    sign_in @user
    @oauth_app = Doorkeeper::Application.create!(name: 'Parity', redirect_uri: 'http://localhost/cb', confidential: false, scopes: 'api mcp')
    ConnectedClient.create!(user: @user, oauth_application: @oauth_app, mcp_tools: @user.enabled_mcp_tool_names,
                            rest_tools: AppConfig::REST_TOOL_DEFAULTS.keys)
    @token = Doorkeeper::AccessToken.create!(application: @oauth_app, resource_owner_id: @user.id, token: SecureRandom.hex(32),
                                             scopes: 'api', expires_in: 3600)
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    SolidQueue::Job.destroy_all
  end

  teardown { ActionMCP::Current.reset }

  [true, false].each do |fresh|
    test "plain restarting bot, start_fresh: #{fresh}: web, MCP and REST start it the same way" do
      travel_to SUNDAY do
        outcomes = start_paths.to_h { |path| [path, start_via(path, plain_bot, fresh)] }
        assert_equal 1, outcomes.values.uniq.size, outcomes.inspect
      end
    end

    test "restarting bot with a starting time, start_fresh: #{fresh}: web, MCP and REST start it the same way" do
      travel_to SUNDAY do
        outcomes = start_paths.to_h { |path| [path, start_via(path, start_time_bot, fresh)] }
        assert_equal 1, outcomes.values.uniq.size, outcomes.inspect

        state = outcomes[:web]
        status, started_at, start_at, ticks = state.values_at(:status, :started_at, :start_at, :ticks)
        assert_equal 'scheduled', status
        if fresh
          monday = Time.utc(2026, 5, 25, 9, 0, 0)
          assert_equal [monday, monday.iso8601, [monday - SUNDAY]], [started_at, start_at, ticks], 'fresh re-anchors on the next Monday open'
        else
          assert_equal [Time.utc(2024, 1, 15, 9), '2024-01-15T09:00:00Z'], [started_at, start_at], 'continue keeps the schedule'
        end
      end
    end
  end

  test 'no start_fresh starts fresh on every path, as the plain Start button does' do
    travel_to SUNDAY do
      outcomes = start_paths.to_h { |path| [path, start_via(path, start_time_bot, nil)] }
      assert_equal 1, outcomes.values.uniq.size, outcomes.inspect
      assert_equal Time.utc(2026, 5, 25, 9), outcomes[:web][:started_at]
    end
  end

  test 'continue carries what was missed, and every path captures the same carry' do
    travel_to SUNDAY do
      outcomes = start_paths.to_h { |path| [path, start_via(path, plain_bot, false)] }
      assert_equal 1, outcomes.values.uniq.size, outcomes.inspect
      assert_operator outcomes[:web][:missed_quote_amount], :>, 0, 'a bot stopped for three days owes its missed intervals'
    end
  end

  # The web asks the wash-sale question before a bot that can sell may start; MCP and REST refuse
  # at the same point with the same words, and none of the three writes or arms anything.
  test 'a bot that can sell is refused on every path while the wash-sale question is unanswered' do
    travel_to SUNDAY do
      message = I18n.t('engine.wash_sale_question_owed', label: 'Seller')
      start_paths.each do |path|
        bot = plain_bot
        bot.update_columns(label: 'Seller', settings: bot.settings.merge('direction' => 'selling'))
        before = state(bot.reload)

        case path
        when :web
          patch bot_start_path(bot_id: bot.id, start_fresh: true), headers: { 'Accept' => TURBO_STREAM_ACCEPT }
          assert_response :unprocessable_entity
          assert_match 'wash_sale[enabled]', response.body, 'the web refusal is the question itself'
          assert_includes CGI.unescapeHTML(response.body), message
        when :mcp
          stub_mcp_client(@user)
          assert_equal message, StartBotTool.new(bot_id: bot.id, start_fresh: true).execute.contents.first.text
        when :rest
          post "/api/v1/bots/#{bot.id}/start", params: { start_fresh: true }, headers: { 'Authorization' => "Bearer #{@token.token}" }, as: :json
          assert_response :unprocessable_entity
          assert_equal %w[wash_sale_question_owed] + [message], JSON.parse(response.body)['error'].values_at('code', 'message')
        end
        assert_equal before, state(bot.reload), "#{path} refused without a write"
      end
    end
  end

  private

  def start_paths = %i[web mcp rest]

  def plain_bot
    bot = create(:dca_single_asset, :hourly, user: @user, exchange: @exchange, base_asset: @btc, quote_asset: @usd,
                                             status: :stopped, started_at: SUNDAY - 3.days)
    bot.update_column(:transient_data, bot.transient_data.merge('last_action_job_at' => (SUNDAY - 20.minutes).iso8601))
    bot.reload
  end

  def start_time_bot
    bot = create(:dca_single_asset, :hourly, user: @user, exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    bot.assign_attributes(status: :stopped, started_at: Time.utc(2024, 1, 15, 9),
                          settings: bot.settings.merge('start_time_enabled' => true, 'start_time_mode' => 'monday',
                                                       'start_time_of_day' => '09:00', 'start_at' => '2024-01-15T09:00:00Z'))
    bot.set_missed_quote_amount
    bot.save!
    bot.update_column(:transient_data, bot.transient_data.merge('last_action_job_at' => (SUNDAY - 20.minutes).iso8601))
    bot.reload
  end

  def start_via(path, bot, fresh)
    flag = fresh.nil? ? {} : { start_fresh: fresh }
    case path
    when :web
      patch bot_start_path(bot_id: bot.id, **flag), headers: { 'Accept' => TURBO_STREAM_ACCEPT }
      assert_response :success
    when :mcp
      stub_mcp_client(@user)
      text = StartBotTool.new(bot_id: bot.id, **flag).execute.contents.first.text
      assert_match(/started successfully/, text)
    when :rest
      post "/api/v1/bots/#{bot.id}/start", params: flag, headers: { 'Authorization' => "Bearer #{@token.token}" }, as: :json
      assert_response :ok
    end
    state(bot.reload)
  end

  # Everything Bot#start writes or arms, ids aside.
  def state(bot)
    { status: bot.status, started_at: bot.started_at, start_at: bot.settings['start_at'], ticks: ticks(bot),
      settings: bot.settings, transient_data: bot.transient_data, settings_changed_at: bot.settings_changed_at,
      stop_message_key: bot.stop_message_key, missed_quote_amount: bot.missed_quote_amount,
      activity: BotActivityLog.where(bot:).order(:id).map { |log| [log.event, log.level, log.details] } }
  end

  # Seconds from now to each Bot::ActionJob armed for this bot.
  def ticks(bot)
    gid = "#{bot.to_global_id}\""
    SolidQueue::Job.where(class_name: 'Bot::ActionJob').order(:id)
                   .select { |job| job.arguments.to_json.include?(gid) }
                   .map { |job| (job.scheduled_at || Time.current) - Time.current }
  end
end
