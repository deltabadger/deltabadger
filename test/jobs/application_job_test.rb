require 'test_helper'

# Records the Time.zone its perform actually ran in.
class ZoneProbeJob < ApplicationJob
  cattr_accessor :observed_zone

  def perform
    self.class.observed_zone = Time.zone.name
  end
end

class ApplicationJobTest < ActiveSupport::TestCase
  # ActiveJob stamps Time.zone into the payload at enqueue and re-applies it around every perform,
  # and a self-rescheduling chain re-stamps whatever zone it ran in — so a single enqueue under a
  # foreign zone pins that zone on the chain for as long as it lives. The app runs on UTC
  # (config.time_zone), and job-side date math must not depend on which zone a chain inherited:
  # under a DST zone, calendar arithmetic silently moves an interval grid by the offset.
  test 'performs in the app zone even when the payload carries another one' do
    ZoneProbeJob.observed_zone = nil
    job = Time.use_zone('Vienna') { ZoneProbeJob.new }
    assert_equal 'Vienna', job.timezone, 'precondition: the payload carries the foreign zone'

    job.perform_now

    assert_equal 'UTC', ZoneProbeJob.observed_zone
    assert_equal 'UTC', Time.zone.name, 'the zone must be restored after the job'
  end

  # retry_on handlers and the retry enqueue itself run OUTSIDE the perform callbacks
  # (rescue_with_handler is called once run_callbacks has unwound), so wrapping perform is not
  # enough: a retried job re-serializes its own timezone attribute and would carry a stale zone
  # onward. Normalizing on deserialize cleans every payload already sitting in the queue.
  test 'a deserialized payload carrying a foreign zone is normalized to the app zone' do
    payload = Time.use_zone('Vienna') { ZoneProbeJob.new.serialize }
    assert_equal 'Vienna', payload['timezone'], 'precondition: the stored payload carries it'

    job = ActiveJob::Base.deserialize(payload)

    assert_equal 'UTC', job.timezone
    assert_equal 'UTC', job.serialize['timezone'], 'a retry must not re-persist the stale zone'
  end
end
