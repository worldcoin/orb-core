$setpoint << EOD
0 1.851
1 1.851
2 1.851
3 1.851
4 1.851
5 1.851
6 1.851
7 1.851
8 1.851
9 1.851
10 1.851
EOD
$process << EOD
0 1.146
1 1.358
2 1.511
3 1.622
4 1.702
5 1.760
6 1.802
7 1.831
8 1.853
9 1.868
10 1.878
EOD
plot "$process" with lines, "$setpoint" with lines
