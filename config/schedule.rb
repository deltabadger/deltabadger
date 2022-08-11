
every 10.minutes do
  rake 'queue_tasks:check_queues'
end