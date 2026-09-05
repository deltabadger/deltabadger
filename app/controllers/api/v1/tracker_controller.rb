# frozen_string_literal: true

module Api
  module V1
    class TrackerController < BaseController
      before_action -> { require_rest_tool!('sync_tracker') }, only: :sync

      def sync
        render_result BotApi::Tracker::Sync.call(user: current_user)
      end
    end
  end
end
