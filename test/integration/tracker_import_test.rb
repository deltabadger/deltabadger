require 'test_helper'

# The import dialog: pick a format, drop a file, done.
#
# ONE step, because there is nothing to decide on a second one — the format is chosen before the
# file is read, the account follows from the format or from the rows, and the time zone comes out of
# the file's own name. The one question that survives is which of two Binance accounts a file came
# from, and only a user who holds both is ever asked it.
class TrackerImportTest < ActionDispatch::IntegrationTest
  BINANCE_NAME = 'Binance-Transaction-History-202608242240(UTC+3)-part1-of1.csv'.freeze
  # Binance writes a UTF-8 byte order mark, and an upload arrives as binary.
  BOM = "\xEF\xBB\xBF".b.freeze
  FILE = [
    'User ID,Time,Account,Operation,Coin,Change,Remark',
    '1,2021-07-01 14:07:04,Spot,Deposit,USDT,150.2,',
    '1,2021-07-01 03:42:38,Spot,Transaction Spend,USDT,-10.0003543,',
    '1,2021-07-01 03:42:38,Spot,Transaction Buy,LTC,0.06983,'
  ].join("\n").freeze

  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user, admin: true, setup_completed: true)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    sign_in @user
  end

  def upload(name: BINANCE_NAME, body: FILE)
    file = Tempfile.new(['import', '.csv']).tap do |f|
      f.binmode
      f.write(body)
      f.rewind
    end
    Rack::Test::UploadedFile.new(file.path, 'text/csv', original_filename: name)
  end

  def deltabadger_csv(*rows)
    ([AccountTransaction.csv_headers.join(',')] + rows).join("\n")
  end

  # ── the dialog ───────────────────────────────────────────────────────────────────────────────
  test 'it offers the formats, opens on our own, and asks nothing else' do
    get new_tracker_import_path

    assert_response :success
    assert_select ".segmented__option[data-value='deltabadger'].is-on"
    assert_select ".segmented__option[data-value='binance']"
    # Not remembered: the dialog opens on the default every time, and the hidden field with it.
    assert_select '.segmented[data-segmented-key]', 0
    # The buttons post nothing on their own; the hidden field is what carries the choice.
    assert_select "input[name=format][value='deltabadger']"
    # Everything else is derived, so nothing else is asked.
    assert_select '[name=api_key_id]', false
    assert_select '[name=offset]', false
  end

  # A label around the input, so the click that opens the file dialog is the browser's own and the
  # zone is reachable by keyboard; the drag handlers are what let a file be dropped on it.
  test 'the file field is a drop zone you can also click' do
    get new_tracker_import_path

    assert_select 'label.dropzone[data-controller=file-picker]' do
      assert_select "input[type=file][data-file-picker-target='input']"
      assert_select '.dropzone__prompt', text: I18n.t('tracker.import.drop')
    end
    assert_select "label.dropzone[data-action*='drop->file-picker#drop']"
  end

  # `.label` is an uppercase 1.5rem chip style — a caption, not a place to put a sentence.
  test 'no step sets prose in the label style' do
    get new_tracker_import_path
    assert_select '.modal p.label', false

    post tracker_import_path, params: { format: 'binance', file: upload(name: 'renamed.csv') }
    assert_select '.modal p.label', false
  end

  # ── one step ─────────────────────────────────────────────────────────────────────────────────
  # Rows landed, so there is nothing left to confirm: the dialog closes, the page comes back with
  # them on it, and the flash is the only sentence.
  test 'a file lands and the dialog gets out of the way' do
    post tracker_import_path, params: { format: 'binance', file: upload }

    assert_response :success
    assert_equal 2, AccountTransaction.for_user(@user).count
    # Into `#flash`, which is `data-turbo-permanent` — the one element the redirect below does not
    # replace. A plain `flash[:notice]` would be wiped by the very visit meant to show it.
    assert_select 'turbo-stream[action=prepend][target=flash]'
    assert_match 'Imported 2 transactions', response.body
    assert_select 'turbo-stream[action=redirect][target="/tracker"]'
    assert_select '.modal', false, 'no second screen to dismiss'
  end

  # The one case that stays put, because it has something to say.
  test 'a file with nothing new in it says so, in the same dialog' do
    post tracker_import_path, params: { format: 'binance', file: upload }

    post tracker_import_path, params: { format: 'binance', file: upload }

    assert_equal 2, AccountTransaction.for_user(@user).count
    assert_select 'turbo-stream[action=redirect]', false, 'nothing landed, so the page does not move'
    assert_select '.modal p', text: I18n.t('tracker.import.nothing_new')
  end

  test "a file carrying Binance's byte order mark is read, not choked on" do
    post tracker_import_path, params: { format: 'binance', file: upload(body: BOM + FILE) }

    assert_response :success
    assert_equal 2, AccountTransaction.for_user(@user).count
  end

  # Choosing the file IS the instruction, so nothing waits for a button.
  test 'the zone sends the file itself' do
    get new_tracker_import_path

    assert_select "input[type=file][data-action*='change->file-picker#submit']"
    assert_select "label.dropzone[data-action*='drop->file-picker#drop']"
    assert_select '.modal input[type=submit]', false
    assert_select '.dropzone__busy'
  end

  # ── the time zone: read, never asked ─────────────────────────────────────────────────────────
  test 'the zone comes from the file name, and the stored instant proves it' do
    post tracker_import_path, params: { format: 'binance', file: upload }

    deposit = AccountTransaction.for_user(@user).find_by(entry_type: :deposit)
    assert_equal Time.utc(2021, 7, 1, 11, 7, 4), deposit.transacted_at, '14:07:04 at UTC+3'
  end

  # A renamed file no longer says which zone its clock was in, and nothing else in it does. Guessing
  # would put the whole history hours out of place and match nothing already stored.
  test 'a Binance file whose name lost its zone is refused, not guessed at' do
    post tracker_import_path, params: { format: 'binance', file: upload(name: 'renamed.csv') }

    assert_response :unprocessable_entity
    assert_equal 0, AccountTransaction.for_user(@user).count
  end

  test 'our own format states an instant, so any name will do' do
    body = deltabadger_csv('2021-07-01T14:07:04Z,deposit,USDT,150.2,,,,,binance,tx-1,,')

    post tracker_import_path, params: { format: 'deltabadger', file: upload(name: 'whatever.csv', body: body) }

    assert_equal Time.utc(2021, 7, 1, 14, 7, 4),
                 AccountTransaction.for_user(@user).sole.transacted_at
  end

  # ── the one question that survives ───────────────────────────────────────────────────────────
  test 'holding both Binance accounts is asked about, and nothing is written until answered' do
    us = create(:api_key, user: @user, exchange: create(:binance_us_exchange))

    post tracker_import_path, params: { format: 'binance', file: upload }

    assert_response :success
    assert_equal 0, AccountTransaction.for_user(@user).count
    assert_select ".segmented__option[data-value='#{@key.id}']"
    assert_select ".segmented__option[data-value='#{us.id}']"

    post tracker_import_path, params: { format: 'binance', api_key_id: us.id,
                                        token: css_select("input[name='token']").first['value'] }

    assert_equal 2, AccountTransaction.for_user(@user).count
    assert_equal [us], AccountTransaction.for_user(@user).map(&:api_key).uniq
    # The parked file kept the zone the first request read off its name.
    assert_equal Time.utc(2021, 7, 1, 11, 7, 4),
                 AccountTransaction.for_user(@user).find_by(entry_type: :deposit).transacted_at
  end

  # ── refusals ─────────────────────────────────────────────────────────────────────────────────
  test 'a file in the wrong format is refused, not half-imported' do
    post tracker_import_path, params: { format: 'deltabadger', file: upload }

    assert_response :unprocessable_entity
    assert_equal 0, AccountTransaction.for_user(@user).count
  end

  # Picking "Binance CSV" IS picking Binance; the refusal is the server's, not the dialog's.
  test 'a Binance file cannot be aimed at another venue by hand' do
    hyperliquid = create(:api_key, user: @user, exchange: create(:hyperliquid_exchange),
                                   raw_key: "0x#{'1' * 40}", raw_secret: '0' * 64)
    @key.destroy

    post tracker_import_path, params: { format: 'binance', api_key_id: hyperliquid.id, file: upload }

    assert_response :unprocessable_entity
    assert_equal 0, AccountTransaction.for_user(@user).count
  end

  test 'an import with no file says so' do
    post tracker_import_path, params: { format: 'binance' }

    assert_response :unprocessable_entity
  end
end
