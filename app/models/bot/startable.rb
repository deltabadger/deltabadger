module Bot::Startable
  extend ActiveSupport::Concern

  # Ordered weekday names. Index + 1 == ISO weekday (Monday = 1 … Sunday = 7),
  # which matches Date#cwday.
  WEEKDAY_MODES = %w[monday tuesday wednesday thursday friday saturday sunday].freeze

  # All valid start_time_mode values: a weekday name, "date", or "hour".
  # The weekday is encoded directly in the mode (so the UI is one flat dropdown).
  # Order here drives the dropdown order — weekdays first (the Monday open is the
  # default), then "date" and "hour". WEEKDAY_MODES keeps its own order untouched
  # because that order encodes the ISO weekday math; only the MODES list is reordered.
  MODES = (WEEKDAY_MODES + %w[date hour]).freeze

  # NYSE regular session opens 09:30 local Eastern time. Using the named zone
  # (not a fixed offset) makes the 09:30 anchor follow EST/EDT automatically.
  NYSE_OPEN_ZONE = 'America/New_York'.freeze

  included do
    store_accessor :settings,
                   :start_time_enabled,
                   :start_time_mode,     # see MODES above
                   :start_time_of_day,   # "HH:MM" interpreted in the bot user's time zone
                   :start_at             # ISO8601 UTC string

    validate :validate_starting_time_settings, on: :start

    decorators = Module.new do
      def parse_params(params)
        parsed = super(params).merge(
          start_time_enabled: cast_bool_for_startable(params[:start_time_enabled]),
          start_time_mode: params[:start_time_mode].presence,
          start_time_of_day: params[:start_time_of_day].presence
        ).compact

        # start_at is special: when the user explicitly submits a value (even a
        # blank or malformed one), overwrite the stored value rather than let
        # .compact drop the key and silently preserve the previous start_at
        # through settings.merge() in BotsController#update.
        parsed[:start_at] = parse_start_at_in_user_zone(params[:start_at]) if params.key?(:start_at)
        parsed
      end
    end

    prepend decorators
  end

  def start_time_enabled?
    ActiveModel::Type::Boolean.new.cast(start_time_enabled) == true
  end

  # The user's chosen display/input time zone (from User#time_zone).
  # Defaults to UTC when the bot has no user or no zone set.
  def user_time_zone
    zone_name = user&.time_zone.presence || 'UTC'
    ActiveSupport::TimeZone[zone_name] || ActiveSupport::TimeZone['UTC']
  end

  # Pure computation: derive the next future first-run timestamp from current settings.
  # Returns nil when feature disabled or settings are malformed.
  # Always returns a UTC Time >= now.
  # Called only at #start time — NOT from interval math.
  def initial_start_at(now: Time.current.utc)
    return nil unless start_time_enabled?

    zone = user_time_zone
    case start_time_mode
    when 'hour'
      compute_hour_anchor(now, zone)
    when *WEEKDAY_MODES
      compute_day_anchor(now, zone, WEEKDAY_MODES.index(start_time_mode) + 1)
    when 'date'
      parse_persisted_start_at
    end
  end

  # Default selection for the starting-time widget: the NYSE Monday open
  # (09:30 ET) expressed in the bot user's local time zone. Because it's a
  # conversion, both the weekday AND the clock time are derived locally — a user
  # east of ~UTC+9.5 correctly defaults to Tuesday in their zone. The ET anchor
  # follows EST/EDT, so the local clock time shifts with US daylight saving.
  # Returns [weekday_mode, "HH:MM"].
  def default_start_time_selection(now: Time.current)
    et = ActiveSupport::TimeZone[NYSE_OPEN_ZONE]
    today_et = now.in_time_zone(et).to_date
    monday = today_et + ((1 - today_et.cwday) % 7) # upcoming Monday (incl. today)
    open_et = et.local(monday.year, monday.month, monday.day, 9, 30)
    local = open_et.in_time_zone(user_time_zone)
    [WEEKDAY_MODES[local.to_date.cwday - 1], local.strftime('%H:%M')]
  end

  # Flips the feature off after the first scheduled execution runs. The
  # "starting time" only affects when the FIRST action fires; once that's
  # happened, the setting has no further effect on scheduling (subsequent
  # runs follow the normal interval cadence from `started_at`). Disabling
  # it post-first-run lets users re-enable with a new value to schedule
  # another delayed start in the future.
  def disable_starting_time!
    return unless start_time_enabled?

    self.start_time_enabled = false
    set_missed_quote_amount
    save!
  end

  # The persisted baseline for interval math. May be in the past once started.
  # Reads settings['start_at'] frozen at #start time — does NOT recompute.
  def repeat_anchor_at
    return read_attribute(:started_at) unless start_time_enabled?

    parse_persisted_start_at || read_attribute(:started_at)
  end

  private

  def cast_bool_for_startable(value)
    return nil if value.nil?

    ActiveModel::Type::Boolean.new.cast(value)
  end

  # The datetime-local <input> submits user-local clock time without a zone.
  # Interpret it in the user's time zone, then store as UTC ISO8601 so the DB
  # value is unambiguous across clients/zones.
  # Out-of-range input (e.g. "2026-99-99T00:00") raises ArgumentError from
  # Time#parse; treat that as nil so it surfaces as a validation error
  # instead of crashing the PATCH /bots/:id action.
  def parse_start_at_in_user_zone(value)
    return nil if value.blank?

    user_time_zone.parse(value)&.utc&.iso8601
  rescue ArgumentError
    nil
  end

  def compute_hour_anchor(now, zone)
    hh, mm = parse_hhmm(start_time_of_day)
    return nil if hh.nil?

    now_in_zone = now.in_time_zone(zone)
    candidate = zone.local(now_in_zone.year, now_in_zone.month, now_in_zone.day, hh, mm).utc
    candidate <= now ? candidate + 1.day : candidate
  end

  def compute_day_anchor(now, zone, weekday)
    hh, mm = parse_hhmm(start_time_of_day)
    return nil if hh.nil?

    now_in_zone = now.in_time_zone(zone)
    today_at_time = zone.local(now_in_zone.year, now_in_zone.month, now_in_zone.day, hh, mm)
    days_ahead = (weekday - now_in_zone.to_date.cwday) % 7

    candidate = (today_at_time + days_ahead.days).utc
    candidate <= now ? candidate + 7.days : candidate
  end

  # Returns [hh, mm] only when both are syntactically valid clock values.
  # Returns [nil, nil] for malformed input — callers must guard so the
  # malformed case becomes a validation error, not a Time.utc exception.
  def parse_hhmm(value)
    return [nil, nil] if value.blank?

    parts = value.to_s.split(':')
    return [nil, nil] if parts.size != 2
    return [nil, nil] unless parts[0].match?(/\A\d{1,2}\z/) && parts[1].match?(/\A\d{1,2}\z/)

    hh = parts[0].to_i
    mm = parts[1].to_i
    return [nil, nil] unless hh.between?(0, 23) && mm.between?(0, 59)

    [hh, mm]
  end

  def parse_persisted_start_at
    value = settings['start_at'].presence
    return nil if value.blank?

    Time.find_zone!('UTC').parse(value)
  rescue ArgumentError
    # Out-of-range date components ("2026-99-99T00:00") raise. Treat as nil so
    # validate_start_at_future can add an error instead of crashing valid?(:start).
    nil
  end

  # Validates settings before scheduling. Bad input must surface as a
  # validation error, NOT an exception from Time.utc(...) and NOT a silent
  # fall-through to the immediate-start path (computed_start_at == nil).
  def validate_starting_time_settings
    return unless start_time_enabled?

    unless MODES.include?(start_time_mode)
      errors.add(:start_time_mode, :inclusion)
      return # later checks depend on a known mode
    end

    case start_time_mode
    when 'hour', *WEEKDAY_MODES
      validate_time_of_day
    when 'date'
      validate_start_at_future
    end
  end

  def validate_time_of_day
    hh, mm = parse_hhmm(start_time_of_day)
    errors.add(:start_time_of_day, :invalid) if hh.nil? || mm.nil?
  end

  def validate_start_at_future
    parsed = parse_persisted_start_at
    if parsed.nil?
      errors.add(:start_at, :blank)
    elsif !parsed.future?
      errors.add(:start_at, :must_be_future)
    end
  end
end
