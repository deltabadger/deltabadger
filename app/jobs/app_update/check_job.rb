# frozen_string_literal: true

# The only thing that asks GitHub. Recurring, twice a day — see config/recurring.yml.
class AppUpdate::CheckJob < ApplicationJob
  queue_as :low_priority

  def perform
    return unless AppUpdate.enabled?

    AppUpdate.refresh!
  end
end
